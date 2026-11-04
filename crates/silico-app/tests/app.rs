//! The app's shell (M5.4; docs/design/m5.0-app.md, sections 3, 4, 8, 9, and 13), driven
//! through `egui_kittest`'s accessibility tree with no GPU and no screenshot (decision 7):
//! each pane found by its label, each control by its role and label, and every frame stepped
//! on a thread marked so that a wait on a command there is counted (the rule is structural,
//! not timed). The sessions are the core's, on the fake kernel. Expected values: the
//! documents' own text and names, written here, and what the core's commands answer of the
//! same documents.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use silico_app::App;
use silico_app::app::IDLE_FRAME;
use silico_app::recent::Recents;
use silico_app::start::Warm;
use silico_app::tab::Sent;
use silico_core::cache::Cache;
use silico_core::command::{CheckAnswer, CheckModel, ModelChoice, Running, Sessions};
use silico_core::disk::{Clock, Found, Reader};
use silico_core::document::json::{self, Plain};
use silico_core::fake::never_waits;
use silico_core::fake::{FakeKernel, FakeSketches, Hold};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use tempfile::TempDir;

/// The M2.1 document's example (docs/design/m2.1-document-format.md): a block, a pin, and
/// the hole the pin cuts in it.
const EXAMPLE: &str = r#"{
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

/// Long enough for anything the fake kernel does on a loaded machine.
const WAIT: Duration = Duration::from_secs(60);

type Fake = Arc<FakeKernel>;
type AppHarness = Harness<'static, App<Fake>>;

struct Fixture {
    dir: TempDir,
    kernel: Fake,
    sketches: FakeSketches,
    sessions: Arc<Sessions<Fake>>,
}

/// Sessions on the fake kernel, each file compared on disk as the app's are.
fn fixture() -> Fixture {
    fixture_reading(None)
}

/// [`fixture`], each file compared through `read` with no interval, if given (a test's
/// reader, to hold a comparison).
fn fixture_reading(read: Option<Reader>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    // STEP written by the fake, as the STEP worker would (M5.12's export dialog).
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    )
    .exporting_step(Arc::clone(&kernel));
    let sessions = match read {
        None => sessions,
        Some(read) => {
            let now: Clock = Arc::new(Instant::now);
            sessions.watching_disk(Duration::ZERO, read, now).unwrap()
        }
    };
    Fixture {
        dir,
        kernel,
        sketches,
        sessions: Arc::new(sessions),
    }
}

impl Fixture {
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The app on these sessions, in a harness, with nothing open.
    fn app(&self) -> AppHarness {
        let app = App::new(Arc::clone(&self.sessions));
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    }

    /// The app with `text` open from `name`, its timeline read.
    fn opened(&self, name: &str, text: &str) -> (AppHarness, PathBuf) {
        let path = self.file(name, text);
        let mut h = self.app();
        h.state_mut().open(path.to_str().unwrap(), false);
        step_until(&mut h, "the timeline read", |h| {
            h.state()
                .tabs()
                .first()
                .is_some_and(|tab| tab.timeline.is_some())
        });
        (h, path)
    }
}

/// One frame, on the marked thread.
fn step(h: &mut AppHarness) {
    let _marked = never_waits::marked();
    h.step();
}

/// Frames stepped until `done` holds, within [`WAIT`].
#[track_caller]
fn step_until(h: &mut AppHarness, what: &str, done: impl Fn(&AppHarness) -> bool) {
    let deadline = Instant::now() + WAIT;
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
fn everything(h: &AppHarness) -> Vec<String> {
    h.root()
        .query_all(egui_kittest::kittest::by().recursive(true))
        .filter_map(|node| {
            let node = node.accesskit_node();
            node.label().or_else(|| node.value())
        })
        .collect()
}

/// The labels of `pane`'s nodes of `role`, in the tree's order.
fn labels(h: &AppHarness, pane: &str, role: Role) -> Vec<String> {
    h.get_by_role_and_label(Role::Pane, pane)
        .query_all_by_role(role)
        .filter_map(|node| {
            let node = node.accesskit_node();
            if role == Role::Label {
                node.value()
            } else {
                node.label()
            }
        })
        .collect()
}

/// Whether `pane` holds a node labelled `label` (a label's text, too).
fn shows(h: &AppHarness, pane: &str, label: &str) -> bool {
    h.get_by_role_and_label(Role::Pane, pane)
        .query_by_label(label)
        .is_some()
}

/// The text of `pane`'s node whose label holds `part`, if one does.
fn showing(h: &AppHarness, pane: &str, part: &str) -> Option<String> {
    h.get_by_role_and_label(Role::Pane, pane)
        .query_all_by_label_contains(part)
        .find_map(|node| {
            let node = node.accesskit_node();
            node.value().or_else(|| node.label())
        })
}

/// Whether the window holds a node labelled `label`, anywhere.
fn anywhere(h: &AppHarness, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

/// The one button labelled `label` that isn't the start page's: the File menu's Open…,
/// while the start page shows its own.
fn button_off_start<'a>(h: &'a AppHarness, label: &'a str) -> egui_kittest::Node<'a> {
    let mut found = h
        .query_all_by_role_and_label(Role::Button, label)
        .filter(|node| !in_pane(node, "Start"));
    let node = found
        .next()
        .unwrap_or_else(|| panic!("no {label} off the start page"));
    assert!(found.next().is_none(), "two {label}s off the start page");
    node
}

/// The feature `name` selected on the timeline, its properties shown.
fn select(h: &mut AppHarness, name: &str) {
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
fn name_field(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::Pane, "Properties")
        .get_by_role(Role::TextInput)
}

/// Typed into the property panel's Name field, one keystroke a frame: focused, the field's
/// cursor is at its start, so what's typed comes before the name.
fn type_name(h: &mut AppHarness, text: &str) {
    name_field(h).focus();
    step(h);
    for c in text.chars() {
        name_field(h).type_text(&c.to_string());
        step(h);
    }
}

fn is_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
}

// ---- Opening, and what the panes show ----

#[test]
fn every_pane_has_its_label() {
    let f = fixture();
    // With no document, the start page alone; with one, the panes.
    let mut h = f.app();
    step(&mut h);
    assert!(h.query_by_role_and_label(Role::Pane, "Start").is_some());
    let (h, _) = f.opened("example.silico", EXAMPLE);
    assert!(h.query_by_role_and_label(Role::Pane, "Start").is_none());
    for pane in ["Browser", "Viewport", "Properties", "Timeline"] {
        assert!(
            h.query_by_role_and_label(Role::Pane, pane).is_some(),
            "{pane}"
        );
    }
}

#[test]
fn opening_the_m2_1_example_shows_its_three_features_in_order_and_its_body() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let features: Vec<String> = labels(&h, "Timeline", Role::Button)
        .into_iter()
        .filter(|label| ["Block", "Pin", "Hole"].contains(&label.as_str()))
        .collect();
    assert_eq!(features, ["Block", "Pin", "Hole"]);
    // The cut is the one body (it consumes the block and the pin), in the Bodies group.
    let browser = h.get_by_role_and_label(Role::Pane, "Browser");
    let bodies = browser.get_by_role_and_label(Role::Group, "Bodies");
    assert!(bodies.query_by_label("Hole").is_some());
    assert!(bodies.query_by_label("Block").is_none());
    assert!(bodies.query_by_label("Pin").is_none());
    // Its origin's planes and axes, and no sketch.
    let origin = browser.get_by_role_and_label(Role::Group, "Origin");
    for name in ["Origin", "X", "Y", "Z", "XY", "XZ", "YZ"] {
        assert!(origin.query_by_label(name).is_some(), "{name}");
    }
    let sketches = browser.get_by_role_and_label(Role::Group, "Sketches");
    assert!(sketches.query_by_role(Role::Button).is_none());
    // Its tab, by its file's name, with nothing unsaved.
    assert!(anywhere(&h, "example.silico"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_failing_feature_is_marked_on_its_timeline_entry_and_opened_says_its_errors_text() {
    let f = fixture();
    f.kernel.refuse(|op| {
        matches!(op, Op::Boolean { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "the cut came apart"))
    });
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    // What the core's own check says of the cut, as an agent reads it.
    let want = format!("error: {}", core_says(&f, EXAMPLE, "m3tq8v0z"));
    // Marked at once; its text read only when the person opens the mark (section 3).
    step_until(&mut h, "the cut's mark", |h| {
        showing(h, "Timeline", "error").is_some_and(|text| text == "error")
    });
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(
        h.state().tabs()[0].said.is_empty(),
        "read before it was opened"
    );
    select(&mut h, "Hole");
    step_until(&mut h, "its text", |h| {
        showing(h, "Timeline", "error:").is_some_and(|text| text == want)
    });
    assert!(shows(&h, "Properties", want.trim_start_matches("error: ")));
    // The browser lists the model's bodies, the block and the pin, whose consumer failed;
    // the cut built none, and the core's timeline reports none for it, so it isn't listed.
    let browser = h.get_by_role_and_label(Role::Pane, "Browser");
    let bodies = browser.get_by_role_and_label(Role::Group, "Bodies");
    assert!(bodies.query_by_label("Block").is_some());
    assert!(bodies.query_by_label("Pin").is_some());
    assert!(bodies.query_by_label("Hole").is_none());
    assert_eq!(std::fs::read_to_string(path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

/// What the core's own check says of `feature` of `text`, opened in a session of its own.
fn core_says(f: &Fixture, text: &str, feature: &str) -> String {
    let name = format!("copy-{feature}.silico");
    let checked = f.sessions.open(f.file(&name, text), false).wait();
    let copy = checked.session.unwrap().id;
    let answer = f
        .sessions
        .check(&copy, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    answer
        .problems
        .errors()
        .iter()
        .map(|error| (error.feature(), error.message().to_owned()))
        .chain(
            answer
                .warnings
                .listed()
                .iter()
                .map(|warning| (warning.feature(), warning.message().to_owned())),
        )
        .find(|(of, _)| *of == Some(feature))
        .map(|(_, message)| message)
        .unwrap()
}

#[test]
fn a_document_the_gate_refuses_opens_with_its_problems_shown() {
    let f = fixture();
    // A cut whose tool names no feature.
    let refused = EXAMPLE.replace(r#""tool": "a91cw0de""#, r#""tool": "zzzzzzzz""#);
    let opened = f
        .sessions
        .open(f.file("copy.silico", &refused), false)
        .wait()
        .result
        .unwrap();
    assert!(opened.problems.total() > 0);
    let (mut h, _) = f.opened("refused.silico", &refused);
    for problem in opened.problems.errors() {
        step_until(&mut h, "the problem shown", |h| {
            shows(h, "Properties", problem.message())
        });
    }
    // As the API opens it: every feature listed, none marked by a model.
    let features: Vec<String> = labels(&h, "Timeline", Role::Button);
    for name in ["Block", "Pin", "Hole"] {
        assert!(features.contains(&name.to_owned()), "{name}: {features:?}");
    }
    // No feature can be made in it: only `replace_text` mends it.
    assert!(
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label("Box")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- The unsaved state: edit, Save, Revert, Close ----

#[test]
fn an_edit_marks_the_document_unsaved_and_save_writes_it() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    select(&mut h, "Hole");
    type_name(&mut h, "X");
    step_until(&mut h, "the tab marked unsaved", |h| {
        anywhere(h, "example.silico *")
            && h.state().tabs()[0].transaction
            && shows(h, "Timeline", "XHole")
    });
    // Nothing written yet: the edit is the transaction's.
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(anywhere(&h, "example.silico *"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    h.get_by_role_and_label(Role::Button, "Save").click();
    step_until(&mut h, "the file saved", |h| {
        anywhere(h, "example.silico")
            && !anywhere(h, "example.silico *")
            && h.state().tabs()[0]
                .timeline
                .as_ref()
                .is_some_and(|timeline| {
                    timeline
                        .features
                        .as_ref()
                        .is_some_and(|features| features.iter().any(|f| f.name == "XHole"))
                })
    });
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains(r#""name": "XHole""#), "{saved}");
    assert!(!saved.contains(r#""name": "Hole","#), "{saved}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn revert_restores_the_files_text_byte_for_byte() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    select(&mut h, "Hole");
    type_name(&mut h, "Y");
    step_until(&mut h, "the tab marked unsaved", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "YHole")
    });
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step_until(&mut h, "reverted", |h| {
        !anywhere(h, "example.silico *") && shows(h, "Timeline", "Hole")
    });
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert_eq!(text, EXAMPLE);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    // The field shows the name reverted to.
    step_until(&mut h, "the field reverted", |h| {
        name_field(h).value().as_deref() == Some("Hole")
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn closing_a_changed_document_asks_and_dont_save_rolls_back_then_closes() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    select(&mut h, "Hole");
    type_name(&mut h, "Z");
    step_until(&mut h, "the tab marked unsaved", |h| {
        anywhere(h, "example.silico *")
    });
    let id = h.state().tabs()[0].id;
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    // Still open while it asks.
    assert!(f.sessions.status(&id).result.is_ok());
    // Through AccessKit's click, as a screen reader's: the modal's buttons are laid over
    // the window.
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step_until(&mut h, "the tab closed", |h| h.state().tabs().is_empty());
    assert!(f.sessions.status(&id).result.is_err(), "the session closed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ten_keystrokes_while_an_edit_waits_make_two_edits_and_the_text_holds_the_last() {
    // Section 3, *Bursts of edits*: at most one in flight; a newer edit of the same field
    // replaces the one waiting.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    select(&mut h, "Hole");
    // A first edit, answered: the transaction begun.
    type_name(&mut h, "a");
    step_until(&mut h, "the first edit answered", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    assert_eq!(h.state().tabs()[0].edits.sent, 1);
    // The session's queue held: a check of the edited revision waits for its rebuild,
    // which the fake holds, and the next edit waits behind it.
    let hold: Hold = f.kernel.hold(|_| true);
    let id = h.state().tabs()[0].id;
    let check = f.sessions.check(&id, CheckModel::Current { fresh: true });
    assert!(f.kernel.wait_running(1, WAIT));
    type_name(&mut h, "bcdefghijk");
    for _ in 0..20 {
        step(&mut h);
    }
    // The first of the ten in flight, the newest waiting: two sent of the ten.
    assert_eq!(h.state().tabs()[0].edits.sent, 2);
    hold.release();
    check.wait().result.unwrap();
    step_until(&mut h, "the edits answered", |h| {
        h.state().tabs()[0].edits.idle()
    });
    assert_eq!(h.state().tabs()[0].edits.sent, 3);
    // The document holds the last value the field held, every keystroke in it.
    let typed = name_field(&h).value().unwrap();
    let mut letters: Vec<char> = typed.replace("Hole", "").chars().collect();
    letters.sort_unstable();
    assert_eq!(letters.into_iter().collect::<String>(), "abcdefghijk");
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(
        text.contains(&format!(r#""name": "{typed}""#)),
        "{typed}: {text}"
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- A file changed on disk ----

#[test]
fn an_agents_commit_reloads_with_no_edits_held_and_is_announced_with_edits_held() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // No edits held: the session reloads it itself, and the app shows it.
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Bore""#),
    )
    .unwrap();
    step_until(&mut h, "the agent's commit shown", |h| {
        shows(h, "Timeline", "Bore") && !shows(h, "Timeline", "Hole")
    });
    assert!(!anywhere(&h, "example.silico *"));
    // Edits held: announced, not reloaded.
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "sPin")
    });
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#);
    std::fs::write(&path, &theirs).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    assert!(shows(&h, "Timeline", "sPin"));
    assert!(shows(&h, "Timeline", "Bore"));
    // Reload: the edits dropped, the agent's file shown, never written over.
    h.get_by_role_and_label(Role::Button, "Reload").click();
    step_until(&mut h, "reloaded", |h| {
        shows(h, "Timeline", "Drill") && !anywhere(h, "example.silico *") && !anywhere(h, said)
    });
    assert!(shows(&h, "Timeline", "Pin"));
    // The field shows the reloaded name, its typed text dropped with the edits.
    step_until(&mut h, "the field reloaded", |h| {
        name_field(h).value().as_deref() == Some("Pin")
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
    assert_eq!(never_waits::waits(), 0);
}

/// The session's own read of its file, as `silico_core::disk` reads it, for a test that
/// compares with no interval.
fn real(path: &Path) -> Found {
    match std::fs::read(path) {
        Ok(bytes) => Found::Bytes(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Gone,
        Err(_) => Found::Unreadable,
    }
}

// ---- Frames keep coming, and the UI thread never waits ----

#[test]
fn frames_climb_and_the_panels_answer_while_a_rebuild_and_its_timeline_read_are_held() {
    let f = fixture();
    let hold = f.kernel.hold(is_box);
    let path = f.file("example.silico", EXAMPLE);
    let mut h = f.app();
    h.state_mut().open(path.to_str().unwrap(), false);
    step_until(&mut h, "the tab", |h| !h.state().tabs().is_empty());
    assert!(f.kernel.wait_running(1, WAIT));
    let before = h.state().frames();
    for _ in 0..100 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), before + 100);
    // The panels answer: the timeline, whose read waits for the rebuild, says so, and the
    // status bar says it's rebuilding.
    assert!(shows(&h, "Timeline", "Building…"));
    assert!(h.state().tabs()[0].timeline.is_none());
    assert!(h.query_by_label_contains("Rebuilding").is_some());
    hold.release();
    step_until(&mut h, "the timeline read", |h| {
        shows(h, "Timeline", "Hole")
    });
    assert_eq!(never_waits::waits(), 0);
}

/// A reader held, while it's held, before it reads.
#[derive(Default)]
struct HeldReader {
    held: Mutex<bool>,
    opened: Condvar,
    reading: AtomicBool,
}

impl HeldReader {
    fn reader(self: &Arc<Self>) -> Reader {
        let me = Arc::clone(self);
        Arc::new(move |path: &Path| {
            me.reading.store(true, Ordering::SeqCst);
            let mut held = me.held.lock().unwrap();
            while *held {
                held = me.opened.wait(held).unwrap();
            }
            drop(held);
            real(path)
        })
    }
}

#[test]
fn frames_climb_and_status_answers_while_a_disk_comparison_is_held() {
    let reader = Arc::new(HeldReader::default());
    let f = fixture_reading(Some(reader.reader()));
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    *reader.held.lock().unwrap() = true;
    reader.reading.store(false, Ordering::SeqCst);
    step_until(&mut h, "a comparison held", |_| {
        reader.reading.load(Ordering::SeqCst)
    });
    let before = h.state().frames();
    for _ in 0..100 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), before + 100);
    assert!(h.state().tabs()[0].status.is_some());
    assert!(shows(&h, "Timeline", "Hole"));
    *reader.held.lock().unwrap() = false;
    reader.opened.notify_all();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_wait_on_the_marked_thread_is_counted() {
    // The rule's own check: a wait there, however short, is caught.
    let f = fixture();
    let path = f.file("example.silico", EXAMPLE);
    let opened = f.sessions.open(&path, false).wait();
    let id = opened.session.unwrap().id;
    assert_eq!(never_waits::waits(), 0);
    {
        let _marked = never_waits::marked();
        let _ = f
            .sessions
            .timeline(&id, ModelChoice::Current)
            .wait_timeout(Duration::from_millis(1));
    }
    assert_eq!(never_waits::waits(), 1);
    // Unmarked again: not counted.
    f.sessions.text(&id, None).wait().result.unwrap();
    assert_eq!(never_waits::waits(), 1);
}

// ---- The waker ----

#[test]
fn the_waker_asks_a_frame_as_a_held_command_answers_and_idle_frames_come_each_interval() {
    let f = fixture();
    let hold = f.kernel.hold(is_box);
    let path = f.file("example.silico", EXAMPLE);
    // Time barely moves from frame to frame, so the frame asked for reads as it was asked.
    let mut h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .with_step_dt(0.001)
            .build_ui_state(
                |ui, app: &mut App<Fake>| app.frame(ui),
                App::new(Arc::clone(&f.sessions)),
            )
    };
    h.state_mut().open(path.to_str().unwrap(), false);
    step_until(&mut h, "the tab", |h| !h.state().tabs().is_empty());
    assert!(f.kernel.wait_running(1, WAIT));
    for _ in 0..5 {
        step(&mut h);
    }
    // Nothing answers while it's held: no wake, and the frame asked for is the idle one,
    // never one at the display's rate.
    let held = h.state().wakes();
    for _ in 0..20 {
        step(&mut h);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(h.state().wakes(), held);
    let delay = |h: &AppHarness| {
        h.output()
            .viewport_output
            .get(&eframe::egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
    };
    let idle = |delay: Option<Duration>| {
        delay.is_some_and(|delay| {
            delay > silico_core::disk::DISK_CHECK_INTERVAL && delay <= IDLE_FRAME
        })
    };
    assert!(idle(delay(&h)), "{:?}", delay(&h));
    // Released: the rebuild's steps and end, and the read's answer, each wake it.
    hold.release();
    step_until(&mut h, "woken", |h| h.state().wakes() > held);
    step_until(&mut h, "the timeline read", |h| {
        shows(h, "Timeline", "Hole")
    });
    // Idle again: the frame asked for is a disk comparison's interval away.
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(idle(delay(&h)), "{:?}", delay(&h));
    assert_eq!(never_waits::waits(), 0);
}

// ---- #298's review ----

/// The names `path`'s file holds, in order.
fn names_in(path: &Path) -> Vec<String> {
    names_of(&std::fs::read_to_string(path).unwrap())
}

/// [`step_until`], the Name field showing `shown` on every frame until then.
fn step_until_showing(
    h: &mut AppHarness,
    what: &str,
    shown: &str,
    done: impl Fn(&AppHarness) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(h);
        assert_eq!(
            name_field(h).value().as_deref(),
            Some(shown),
            "the field, before {what}"
        );
        if done(h) {
            return;
        }
        assert!(Instant::now() < deadline, "never {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Frames stepped until `done` holds, however long that takes: for a test that waits on
/// an event (the window's close, a save's bytes), never on the wall clock, so a loaded
/// machine only slows it, and one where the event never comes fails at nextest's own
/// timeout (.config/nextest.toml).
fn step_until_event(h: &mut AppHarness, done: impl Fn(&AppHarness) -> bool) {
    while !done(h) {
        step(h);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The queue of the session of `h`'s tab `at` held behind a check whose rebuild the fake
/// holds (every step built anew), until the hold is released: an edit sent now waits.
fn hold_queue(f: &Fixture, h: &AppHarness, at: usize) -> (Hold, Running<CheckAnswer>) {
    let hold = f.kernel.hold(|_| true);
    let id = h.state().tabs()[at].id;
    let check = f.sessions.check(&id, CheckModel::Current { fresh: true });
    assert!(f.kernel.wait_running(1, WAIT));
    (hold, check)
}

/// The window asked to close, as its close button asks.
fn close_window(h: &mut AppHarness) {
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

/// Whether the last frame told the window to close, or to cancel a close.
fn told(h: &AppHarness, close: bool) -> bool {
    let want = if close {
        eframe::egui::ViewportCommand::Close
    } else {
        eframe::egui::ViewportCommand::CancelClose
    };
    h.output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .is_some_and(|viewport| viewport.commands.contains(&want))
}

#[test]
fn the_close_questions_save_writes_the_edit_then_closes() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    select(&mut h, "Hole");
    type_name(&mut h, "S");
    step_until(&mut h, "unsaved", |h| h.state().tabs()[0].transaction);
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    // The question's Save, not the application bar's.
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "the tab closed", |h| h.state().tabs().is_empty());
    assert_eq!(names_in(&path), ["Block", "Pin", "SHole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_and_close_saves_every_edit_made_before_it_even_one_still_waiting() {
    // "Bl", then "Blo" while "Bl" waits behind a held command: the save waits for both, so
    // the file holds "Blo", and the close comes after.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    select(&mut h, "Block");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    // Two keystrokes while the queue is held: the first's edit in flight, the second's
    // waiting. (The field's cursor stays at its start in the harness, so each keystroke
    // goes before the last: the value it held last is what's asserted.)
    type_name(&mut h, "l");
    let first = name_field(&h).value().unwrap();
    type_name(&mut h, "o");
    let last = name_field(&h).value().unwrap();
    assert_ne!(first, last);
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    for _ in 0..10 {
        step(&mut h);
    }
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the tab closed", |h| h.state().tabs().is_empty());
    assert_eq!(names_in(&path), [last.as_str(), "Pin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn quitting_with_two_changed_documents_and_save_all_saves_both_then_closes_the_window() {
    let f = fixture();
    let (mut h, a) = f.opened("a.silico", EXAMPLE);
    let b = f.file("b.silico", EXAMPLE);
    h.state_mut().open(b.to_str().unwrap(), false);
    step_until(&mut h, "two tabs", |h| {
        h.state().tabs().len() == 2 && h.state().tabs()[1].timeline.is_some()
    });
    for name in ["a.silico", "b.silico"] {
        h.get_by_label(name).click();
        step(&mut h);
        step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
        select(&mut h, "Pin");
        type_name(&mut h, "Q");
        step_until(&mut h, "unsaved", |h| anywhere(h, &format!("{name} *")));
    }
    close_window(&mut h);
    assert!(told(&h, false), "a close with edits held wasn't cancelled");
    step_until(&mut h, "the question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    // The saves held behind a check of the first: a second close while they run is
    // cancelled too, the quit closing the window only once every tab has gone.
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_role_and_label(Role::Button, "Save all")
        .click_accesskit();
    step(&mut h);
    step(&mut h);
    assert!(!h.state().tabs().is_empty());
    close_window(&mut h);
    assert!(
        told(&h, false),
        "a close while the saves ran wasn't cancelled"
    );
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    for path in [&a, &b] {
        assert_eq!(names_in(path), ["Block", "QPin", "Hole"]);
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_quit_whose_save_is_a_conflict_keeps_that_document_and_the_window_open() {
    let f = fixture();
    let (mut h, a) = f.opened("a.silico", EXAMPLE);
    let b = f.file("b.silico", EXAMPLE);
    h.state_mut().open(b.to_str().unwrap(), false);
    step_until(&mut h, "two tabs", |h| {
        h.state().tabs().len() == 2 && h.state().tabs()[1].timeline.is_some()
    });
    for name in ["a.silico", "b.silico"] {
        h.get_by_label(name).click();
        step(&mut h);
        step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
        select(&mut h, "Pin");
        type_name(&mut h, "Q");
        step_until(&mut h, "unsaved", |h| anywhere(h, &format!("{name} *")));
    }
    // An agent's commit to a.silico meanwhile: its save is a conflict.
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Theirs""#);
    std::fs::write(&a, &theirs).unwrap();
    close_window(&mut h);
    step_until(&mut h, "the question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Save all")
        .click_accesskit();
    step_until(&mut h, "b closed and the quit stopped", |h| {
        h.state().tabs().len() == 1
            && h.query_by_label_contains("Quit stopped: a.silico")
                .is_some()
    });
    for _ in 0..20 {
        step(&mut h);
        assert!(
            !told(&h, true),
            "the window closed with a.silico's edits held"
        );
    }
    assert_eq!(h.state().tabs()[0].name(), "a.silico");
    assert!(h.state().tabs()[0].transaction, "a.silico's edits dropped");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), theirs);
    assert_eq!(names_in(&b), ["Block", "QPin", "Hole"]);
    // A second close of the window asks again: never closes it silently.
    close_window(&mut h);
    assert!(told(&h, false));
    step_until(&mut h, "asked again", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the question gone", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_none()
    });
    // Its tab closed by hand, without saving: the window stays, the quit having stopped.
    h.get_by_label("Close a.silico").click();
    step_until(&mut h, "the close question", |h| {
        anywhere(h, "Save changes to a.silico before closing?")
    });
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step_until(&mut h, "a closed", |h| h.state().tabs().is_empty());
    for _ in 0..5 {
        step(&mut h);
        assert!(!told(&h, true), "the stopped quit closed the window");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn closing_the_first_of_three_tabs_closes_that_document_whatever_is_shown() {
    let f = fixture();
    let (mut h, first) = f.opened("one.silico", EXAMPLE);
    for name in ["two.silico", "three.silico"] {
        let path = f.file(name, EXAMPLE);
        h.state_mut().open(path.to_str().unwrap(), false);
    }
    step_until(&mut h, "three tabs", |h| h.state().tabs().len() == 3);
    h.get_by_label("one.silico").click();
    step(&mut h);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "W");
    step_until(&mut h, "unsaved", |h| anywhere(h, "one.silico *"));
    // The third shown, then the first's close asked: the question is the first's.
    h.get_by_label("three.silico").click();
    step(&mut h);
    h.get_by_label("Close one.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to one.silico before closing?")
    });
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step_until(&mut h, "the first closed", |h| h.state().tabs().len() == 2);
    let names: Vec<&str> = h.state().tabs().iter().map(|tab| tab.name()).collect();
    assert_eq!(names, ["two.silico", "three.silico"]);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn undo_restores_the_text_byte_for_byte_and_redo_the_edit() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "A");
    step_until(&mut h, "one edit", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "APin")
    });
    let id = h.state().tabs()[0].id;
    let after_one = f.sessions.text(&id, None).wait().result.unwrap().text;
    type_name(&mut h, "B");
    step_until(&mut h, "two edits", |h| shows(h, "Timeline", "BAPin"));
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step_until(&mut h, "undone", |h| shows(h, "Timeline", "APin"));
    assert_eq!(
        f.sessions.text(&id, None).wait().result.unwrap().text,
        after_one
    );
    // The field follows the document back (its typed text no longer ahead of it).
    step_until(&mut h, "the field undone", |h| {
        name_field(h).value().as_deref() == Some("APin")
    });
    h.get_by_role_and_label(Role::Button, "Redo").click();
    step_until(&mut h, "redone", |h| shows(h, "Timeline", "BAPin"));
    step_until(&mut h, "the field redone", |h| {
        name_field(h).value().as_deref() == Some("BAPin")
    });
    // The next keystroke goes before the document's name as it is now.
    type_name(&mut h, "C");
    step_until(&mut h, "the next edit", |h| shows(h, "Timeline", "CBAPin"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_s_saves() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    step_until(&mut h, "unsaved", |h| h.state().tabs()[0].transaction);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "saved", |h| !anywhere(h, "example.silico *"));
    assert_eq!(names_in(&path), ["Block", "KPin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_stops_the_command_waiting_on_a_held_rebuild() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let (hold, check) = hold_queue(&f, &h, 0);
    step_until(&mut h, "the check running", |h| {
        h.state().tabs()[0]
            .status
            .as_ref()
            .and_then(|status| status.running)
            .is_some()
    });
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    let answered = check.wait_timeout(WAIT).ok().unwrap();
    assert_eq!(
        answered.result.unwrap_err().errors()[0].code(),
        ErrorCode::Cancelled
    );
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn keep_my_edits_hides_the_banner_and_the_save_is_then_a_conflict() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "E");
    step_until(&mut h, "unsaved", |h| h.state().tabs()[0].transaction);
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Theirs""#);
    std::fs::write(&path, &theirs).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the banner", |h| anywhere(h, said));
    h.get_by_role_and_label(Role::Button, "Keep my edits")
        .click();
    step(&mut h);
    for _ in 0..30 {
        step(&mut h);
        assert!(!anywhere(&h, said), "the banner came back");
    }
    h.get_by_role_and_label(Role::Button, "Save").click();
    step_until(&mut h, "the conflict said", |h| {
        h.query_by_label_contains("changed outside this session")
            .is_some()
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
    assert!(h.state().tabs()[0].transaction);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_file_deleted_with_edits_held_is_announced_as_gone() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "G");
    step_until(&mut h, "unsaved", |h| h.state().tabs()[0].transaction);
    std::fs::remove_file(&path).unwrap();
    step_until(&mut h, "the banner", |h| {
        anywhere(
            h,
            "example.silico is gone from disk while you have unsaved edits.",
        )
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_of_another_feature_never_replaces_one_waiting_and_both_land() {
    // Two keystrokes into Block while the queue is held (one in flight, one waiting), then
    // one into Pin: Block holds its last value, Pin its own.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Block");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "1");
    type_name(&mut h, "2");
    let block = name_field(&h).value().unwrap();
    select(&mut h, "Pin");
    type_name(&mut h, "z");
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "all answered", |h| h.state().tabs()[0].edits.idle());
    h.get_by_role_and_label(Role::Button, "Save").click();
    step_until(&mut h, "saved", |h| !anywhere(h, "example.silico *"));
    assert_eq!(names_in(&path), [block.as_str(), "zPin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn two_documents_each_show_their_own_features() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    let other = f.file(
        "other.silico",
        &EXAMPLE
            .replace(r#""name": "Block""#, r#""name": "Plate""#)
            .replace(r#""name": "Hole""#, r#""name": "Bore""#),
    );
    h.state_mut().open(other.to_str().unwrap(), false);
    step_until(&mut h, "the other's features", |h| {
        shows(h, "Timeline", "Bore")
    });
    assert!(!shows(&h, "Timeline", "Hole"));
    h.get_by_label("example.silico").click();
    step_until(&mut h, "the first's features", |h| {
        shows(h, "Timeline", "Hole")
    });
    assert!(!shows(&h, "Timeline", "Bore"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn new_through_its_dialog_makes_the_file_and_open_cancelled_opens_nothing() {
    let f = fixture();
    let mut h = f.app();
    h.get_by_label("File").click();
    step(&mut h);
    button_off_start(&h, "Open…").click();
    step_until(&mut h, "the dialog", |h| anywhere(h, "Open document"));
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| !anywhere(h, "Open document"));
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(h.state().tabs().is_empty());
    let made = f.dir.path().join("new.silico");
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "New…").click();
    step_until(&mut h, "the dialog", |h| anywhere(h, "New document"));
    h.get_by_role(Role::TextInput).focus();
    step(&mut h);
    h.get_by_role(Role::TextInput)
        .type_text(made.to_str().unwrap());
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Create")
        .click_accesskit();
    step_until(&mut h, "its tab", |h| anywhere(h, "new.silico"));
    assert!(made.is_file());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_sketchs_warning_is_marked_in_the_sketches_group_and_its_eye_hides_it() {
    // A sketch whose rectangle's last side ends at a point of its own where the first
    // begins (`ends_not_joined`, a warning), and a pad of it.
    const SKETCHED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Profile", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 4, "y": 0, "construction": false}, {"id": "pa000003", "x": 4, "y": 2, "construction": false}, {"id": "pa000004", "x": 0, "y": 2, "construction": false}, {"id": "pa000005", "x": 0, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}]}"#;
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    let _ = &f.sketches;
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Profile"));
    let sketches = || {
        h.get_by_role_and_label(Role::Pane, "Browser")
            .get_by_role_and_label(Role::Group, "Sketches")
            .query_all_by_label_contains("warning")
            .count()
    };
    assert_eq!(sketches(), 1);
    // Opened: its text, the core's.
    let want = format!("warning: {}", core_says(&f, SKETCHED, "s0000001"));
    select(&mut h, "Profile");
    step_until(&mut h, "its text", |h| {
        h.get_by_role_and_label(Role::Pane, "Browser")
            .get_by_role_and_label(Role::Group, "Sketches")
            .query_by_label(&want)
            .is_some()
    });
    h.get_by_label("Hide Profile").click();
    step(&mut h);
    step(&mut h);
    assert!(anywhere(&h, "Show Profile"));
    assert!(!anywhere(&h, "Hide Profile"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_waiting_command_on_another_marked_thread_isnt_this_ones() {
    // Counted for the thread that waited, never for another test's.
    let f = fixture();
    let path = f.file("example.silico", EXAMPLE);
    let id = f.sessions.open(&path, false).wait().session.unwrap().id;
    let sessions = Arc::clone(&f.sessions);
    let there = std::thread::spawn(move || {
        let _marked = never_waits::marked();
        sessions.text(&id, None).wait().result.unwrap();
        never_waits::waits()
    })
    .join()
    .unwrap();
    assert_eq!(there, 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn stop_says_a_cancel_the_core_refuses() {
    // Asked to stop a rebuild that has ended since status said it ran: the core's refusal
    // is said in the tab.
    use silico_app::tab::Tab;
    use silico_core::command::{RebuildState, RunningCommand, SessionCommand};
    let f = fixture();
    let path = f.file("example.silico", EXAMPLE);
    let opened = f.sessions.open(&path, false).wait();
    let id = opened.session.unwrap().id;
    let mut tab = Tab::opened(
        id,
        path.to_str().unwrap().to_owned(),
        &opened.result.unwrap().problems,
    );
    let mut status = f.sessions.status(&id).result.unwrap();
    status.rebuild.state = RebuildState::Running;
    tab.status = Some(status);
    assert!(tab.stoppable());
    tab.stop(&f.sessions);
    let said = tab.message.clone().unwrap();
    assert!(said.contains("no rebuild is running in session"), "{said}");
    // Esc's stop, or a dialog preview's, too late says nothing: there was nothing left to
    // stop. With a selection, Esc clears it first, and stops nothing.
    tab.message = None;
    tab.stop_quietly(&f.sessions);
    assert_eq!(tab.message, None);
    tab.escape(&f.sessions);
    assert_eq!(tab.message, None);
    tab.selected = Some(silico_core::document::FeatureId::new("a91cw0de").unwrap());
    tab.escape(&f.sessions);
    assert_eq!(tab.selected, None);
    assert_eq!(tab.message, None);
    // No rebuild running: the stop off, whatever command runs (a command with no rebuild
    // is its own dialog's to stop), and the session's own reload among them; in every
    // state but running.
    for state in [
        RebuildState::Idle,
        RebuildState::Done,
        RebuildState::Faulted,
        RebuildState::Stopped,
    ] {
        for running in [
            Some(RunningCommand::Command(SessionCommand::Check)),
            Some(RunningCommand::Command(SessionCommand::Export)),
            Some(RunningCommand::Reload),
            None,
        ] {
            if let Some(status) = &mut tab.status {
                status.rebuild.state = state;
                status.running = running;
            }
            assert!(!tab.stoppable(), "{state:?} {running:?}");
        }
    }
}

#[test]
fn a_name_emptied_in_the_field_is_refused_and_the_field_keeps_what_was_typed() {
    // The property panel sends every change of its Name, an empty one too, for the core
    // to answer (the cursor at its start, Delete takes one character each). The refusal is
    // said, and the field keeps "", what the person typed (#298's review).
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    empty_name(&mut h);
    // "in", "n", then "" (a name queued behind one in flight is replaced by the next, so
    // "n" may never be sent): "" refused by the gate, its words said in the status bar, the
    // document holding the last name applied before it, "in" or "n".
    assert_eq!(
        h.state().tabs()[0].message.as_deref(),
        Some("name is empty (give the feature a name)")
    );
    assert!(anywhere(&h, "name is empty (give the feature a name)"));
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(
        text.contains(r#""name": "n""#) || text.contains(r#""name": "in""#),
        "{text}"
    );
    assert_eq!(name_field(&h).value().as_deref(), Some(""));
    assert_eq!(never_waits::waits(), 0);
}

/// Pin's Name emptied with Delete (the cursor at its start), and "" refused.
fn empty_name(h: &mut AppHarness) {
    name_field(h).focus();
    step(h);
    for _ in 0.."Pin".len() {
        h.key_press(eframe::egui::Key::Delete);
        step(h);
    }
    // Answered once nothing is in flight or queued and the refusal of "" is said: how
    // many edits went is the tab's to coalesce (one in flight replaces none, the next
    // queued replaces the one queued before it), so it isn't counted.
    step_until(h, "the empty name answered", |h| {
        h.state().tabs()[0].edits.idle()
            && h.state().tabs()[0].message.as_deref()
                == Some("name is empty (give the feature a name)")
    });
    for _ in 0..20 {
        step(h);
    }
}

#[test]
fn a_name_emptied_then_typed_is_what_was_typed() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    empty_name(&mut h);
    // Typed at once (one text event), so the harness's cursor doesn't reorder it.
    name_field(&h).type_text("Bolt");
    step(&mut h);
    step_until(&mut h, "Bolt", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "Bolt")
    });
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains(r#""name": "Bolt""#), "{text}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_window_for_missing_workers_says_each_and_quits() {
    use silico_app::missing::Missing;
    let lines = vec![
        "there's no sketch worker, so no sketch can be solved: here".to_owned(),
        "there's no STEP worker, so nothing can be exported as STEP: there".to_owned(),
    ];
    let mut h = Harness::builder().with_size((800.0, 400.0)).build_ui_state(
        |ui, app: &mut Missing| app.frame(ui),
        Missing::new(lines.clone()),
    );
    for line in &lines {
        assert!(h.query_by_label(line).is_some(), "{line}");
    }
    h.get_by_role_and_label(Role::Button, "Quit").click();
    h.step();
    assert!(
        h.output()
            .viewport_output
            .get(&eframe::egui::ViewportId::ROOT)
            .is_some_and(|viewport| viewport
                .commands
                .contains(&eframe::egui::ViewportCommand::Close))
    );
}

#[test]
fn revert_drops_an_edit_still_waiting_so_nothing_is_saved_after_it() {
    // One edit in flight behind a held command, the next waiting, then Revert: the waiting
    // one is dropped, never sent once the rollback has ended the transaction (where it
    // would be saved at once).
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    type_name(&mut h, "z");
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "reverted", |h| {
        !h.state().tabs()[0].transaction && h.state().tabs()[0].edits.idle()
    });
    for _ in 0..30 {
        step(&mut h);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    let id = h.state().tabs()[0].id;
    assert_eq!(
        f.sessions.text(&id, None).wait().result.unwrap().text,
        EXAMPLE
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_made_while_a_save_runs_is_sent_after_it_in_a_new_transaction() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "a");
    step_until(&mut h, "the edit answered", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    // The save held behind the session's queue, a keystroke made while it waits.
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_role_and_label(Role::Button, "Save").click();
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    type_name(&mut h, "b");
    let typed = name_field(&h).value().unwrap();
    for _ in 0..10 {
        step(&mut h);
    }
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the keystroke applied", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", &typed)
    });
    // In the document, unsaved, after the save that wrote the first.
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains(&format!(r#""name": "{typed}""#)), "{text}");
    assert_eq!(names_in(&path), ["Block", "aPin", "Hole"]);
    step_until(&mut h, "unsaved", |h| anywhere(h, "example.silico *"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_z_while_quits_save_waits_never_drops_the_save_that_closes() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    // An edit in flight, so the quit's save waits behind it.
    type_name(&mut h, "y");
    let typed = name_field(&h).value().unwrap();
    close_window(&mut h);
    step_until(&mut h, "the question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Save all")
        .click_accesskit();
    step(&mut h);
    step(&mut h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert_eq!(names_in(&path), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn undo_waits_for_the_edits_made_before_it() {
    // Two keystrokes held (one in flight, one waiting), then Undo: it undoes the last of
    // them, once both have answered, so the document holds the first's value.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "1");
    let first = name_field(&h).value().unwrap();
    type_name(&mut h, "2");
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "undone", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", &first)
    });
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains(&format!(r#""name": "{first}""#)), "{text}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_close_question_stays_with_its_document_when_another_tab_closes_meanwhile() {
    let f = fixture();
    let (mut h, _) = f.opened("one.silico", EXAMPLE);
    let mut paths = Vec::new();
    for name in ["two.silico", "three.silico"] {
        let path = f.file(name, EXAMPLE);
        h.state_mut().open(path.to_str().unwrap(), false);
        paths.push(path);
    }
    step_until(&mut h, "three tabs", |h| h.state().tabs().len() == 3);
    h.get_by_label("three.silico").click();
    step(&mut h);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "W");
    step_until(&mut h, "unsaved", |h| anywhere(h, "three.silico *"));
    h.get_by_label("Close three.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to three.silico before closing?")
    });
    // The first closes while the question is open (by an agent's hand, say).
    let one = h.state().tabs()[0].id;
    f.sessions.close(&one).wait().result.unwrap();
    step_until(&mut h, "one gone", |h| h.state().tabs().len() == 2);
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step_until(&mut h, "three closed", |h| h.state().tabs().len() == 1);
    assert_eq!(h.state().tabs()[0].name(), "two.silico");
    assert_eq!(std::fs::read_to_string(&paths[1]).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_second_ctrl_s_while_a_save_runs_is_ignored() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    for _ in 0..10 {
        step(&mut h);
    }
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "saved", |h| !anywhere(h, "example.silico *"));
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].saves, 1);
    assert!(h.state().tabs()[0].message.is_none());
    assert_eq!(names_in(&path), ["Block", "sPin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_typed_after_ctrl_z_comes_after_the_undo() {
    // E1 in flight behind a held command, Ctrl+Z, then E2: the undo undoes E1, then E2 is
    // applied, so the document holds E2's value.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let before = h.state().tabs()[0].revision.number();
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "1");
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    type_name(&mut h, "2");
    let second = name_field(&h).value().unwrap();
    hold.release();
    let _ = check.wait();
    step_until_showing(&mut h, "all answered", &second, |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", &second)
    });
    for _ in 0..20 {
        step(&mut h);
        assert_eq!(name_field(&h).value().as_deref(), Some(second.as_str()));
    }
    // E1, the undo, E2: three revisions, the undo among them.
    assert_eq!(h.state().tabs()[0].revision.number(), before + 3);
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains(&format!(r#""name": "{second}""#)), "{text}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_quit_while_a_save_runs_closes_once_it_has_saved() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until_event(&mut h, |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "q");
    step_until_event(&mut h, |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until_event(&mut h, |h| h.state().tabs()[0].saves == 1);
    close_window(&mut h);
    step_until_event(&mut h, |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Save all")
        .click_accesskit();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until_event(&mut h, |h| told(h, true));
    assert_eq!(names_in(&path), ["Block", "qPin", "Hole"]);
    assert!(h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_second_save_after_new_typing_saves_it() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "a");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    type_name(&mut h, "b");
    let typed = name_field(&h).value().unwrap();
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "both saved", |h| {
        h.state().tabs()[0].saves == 2
            && h.state().tabs()[0].edits.idle()
            && !anywhere(h, "example.silico *")
    });
    assert_eq!(names_in(&path), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_name_field_is_off_while_its_tab_closes() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "c");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    step(&mut h);
    assert!(h.state().tabs()[0].closing(), "not closing");
    assert!(name_field(&h).accesskit_node().is_disabled());
    // Nor can a dialog open: what it sent would come after the close.
    assert!(
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label("Box")
            .accesskit_node()
            .is_disabled()
    );
    let before = name_field(&h).value();
    name_field(&h).type_text("z");
    step(&mut h);
    assert_eq!(name_field(&h).value(), before);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "closed", |h| h.state().tabs().is_empty());
    assert_eq!(names_in(&path), ["Block", "cPin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn typing_while_an_undo_runs_comes_after_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "a");
    step_until(&mut h, "the edit answered", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let before = h.state().tabs()[0].revision.number();
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step(&mut h);
    step(&mut h);
    type_name(&mut h, "u");
    let typed = name_field(&h).value().unwrap();
    hold.release();
    let _ = check.wait();
    step_until_showing(&mut h, "the typing applied", &typed, |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", &typed)
    });
    for _ in 0..20 {
        step(&mut h);
        assert_eq!(name_field(&h).value().as_deref(), Some(typed.as_str()));
    }
    // The undo, then the typing: two revisions.
    assert_eq!(h.state().tabs()[0].revision.number(), before + 2);
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains(&format!(r#""name": "{typed}""#)), "{text}");
    assert!(h.state().tabs()[0].message.is_none());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn typing_while_a_revert_runs_comes_after_it_in_a_new_transaction() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "a");
    step_until(&mut h, "the edit answered", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step(&mut h);
    step(&mut h);
    type_name(&mut h, "r");
    let typed = name_field(&h).value().unwrap();
    hold.release();
    let _ = check.wait();
    step_until_showing(&mut h, "the typing applied", &typed, |h| {
        h.state().tabs()[0].edits.idle()
            && h.state().tabs()[0].transaction
            && shows(h, "Timeline", &typed)
    });
    for _ in 0..20 {
        step(&mut h);
        assert_eq!(name_field(&h).value().as_deref(), Some(typed.as_str()));
    }
    assert!(anywhere(&h, "example.silico *"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_revert_after_a_quits_save_waits_behind_it_and_the_quit_closes() {
    // The quit's save queued behind an edit in flight, then File > Revert: the save stays
    // ahead of the revert, saves both edits and closes the tab, and the revert has nothing
    // left to do.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    // An edit in flight, so the quit's save waits queued behind it.
    type_name(&mut h, "y");
    let typed = name_field(&h).value().unwrap();
    close_window(&mut h);
    step_until(&mut h, "the question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Save all")
        .click_accesskit();
    step(&mut h);
    step(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    assert_eq!(names_in(&path), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_held_behind_a_refused_save_lands_in_the_open_transaction() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "a");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_role_and_label(Role::Button, "Save").click();
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    type_name(&mut h, "e");
    let typed = name_field(&h).value().unwrap();
    // An agent's commit meanwhile: the save is a conflict.
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Theirs""#);
    std::fs::write(&path, &theirs).unwrap();
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the edit applied", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", &typed)
    });
    assert!(h.state().tabs()[0].transaction);
    assert!(
        h.query_by_label_contains("changed outside this session")
            .is_some(),
        "the conflict wasn't said"
    );
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains(&format!(r#""name": "{typed}""#)), "{text}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
    assert_eq!(never_waits::waits(), 0);
}

/// Pin renamed "xPin" and saved, then typed again ("yxPin"), the field having typed both
/// names.
fn saved_then_typed(h: &mut AppHarness) {
    step_until(h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(h, "Pin");
    type_name(h, "x");
    step_until(h, "xPin", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "xPin")
    });
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(h, "saved", |h| {
        h.state().tabs()[0].saves == 1 && !anywhere(h, "example.silico *")
    });
    type_name(h, "y");
    step_until(h, "yxPin held", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "yxPin")
    });
}

#[test]
fn revert_to_a_name_the_field_typed_before_shows_that_name() {
    // The name reverted to is one the field typed ("xPin", saved), so only the revert's
    // reset, not the name, says the field's "yxPin" no longer stands.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    saved_then_typed(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step_until(&mut h, "reverted", |h| {
        !anywhere(h, "example.silico *") && shows(h, "Timeline", "xPin")
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(name_field(&h).value().as_deref(), Some("xPin"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn reload_to_a_name_the_field_typed_before_shows_that_name() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    saved_then_typed(&mut h);
    let theirs = std::fs::read_to_string(&path)
        .unwrap()
        .replace(r#""name": "Hole""#, r#""name": "Drill""#);
    assert!(theirs.contains(r#""name": "xPin""#), "{theirs}");
    std::fs::write(&path, &theirs).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    h.get_by_role_and_label(Role::Button, "Reload").click();
    step_until(&mut h, "reloaded", |h| {
        shows(h, "Timeline", "Drill") && !anywhere(h, "example.silico *")
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(name_field(&h).value().as_deref(), Some("xPin"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_revert_after_a_queued_save_keeps_the_save_and_has_nothing_left_to_revert() {
    // A save waiting (its session's queue held), then "b" typed and Ctrl+S again, both queued, then
    // File > Revert: the save queued stays ahead of the revert and saves "b"; the revert
    // comes up with no transaction open and is done, refusing nothing.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "a");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    type_name(&mut h, "b");
    let typed = name_field(&h).value().unwrap();
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "both saved", |h| {
        h.state().tabs()[0].saves == 2 && !h.state().tabs()[0].unsaved()
    });
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(names_in(&path), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(h.state().tabs()[0].message, None);
    assert!(!anywhere(&h, "example.silico *"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn dont_save_on_a_quit_while_a_save_runs_closes_once_it_has_saved() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "q");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    close_window(&mut h);
    step_until(&mut h, "the question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the window closing", |h| told(h, true));
    // The save asked before the quit stands.
    assert_eq!(names_in(&path), ["Block", "qPin", "Hole"]);
    assert!(h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_sessions_own_reload_to_a_name_the_field_typed_before_shows_that_name() {
    // Two saves, "xPin" then "yxPin", the field having typed both; then an agent writes
    // "xPin" back with no transaction open: the session reloads it itself, and the field
    // shows it, not the "yxPin" it last typed.
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    for (key, name, saves) in [("x", "xPin", 1), ("y", "yxPin", 2)] {
        type_name(&mut h, key);
        step_until(&mut h, name, |h| {
            h.state().tabs()[0].edits.idle() && shows(h, "Timeline", name)
        });
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
        step_until(&mut h, "saved", |h| {
            h.state().tabs()[0].saves == saves && !h.state().tabs()[0].unsaved()
        });
    }
    let theirs = std::fs::read_to_string(&path)
        .unwrap()
        .replace(r#""name": "yxPin""#, r#""name": "xPin""#);
    assert!(theirs.contains(r#""name": "xPin""#), "{theirs}");
    std::fs::write(&path, &theirs).unwrap();
    step_until(&mut h, "the agent's name shown", |h| {
        shows(h, "Timeline", "xPin") && !shows(h, "Timeline", "yxPin")
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(name_field(&h).value().as_deref(), Some("xPin"));
    assert!(!anywhere(&h, "example.silico *"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_s_right_after_the_first_keystroke_saves_it() {
    // The first edit's `begin` held, so no transaction is open yet: Save is offered all the
    // same (the tab is unsaved), and Ctrl+S saves the edit once it's made.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "s");
    step(&mut h);
    assert!(!h.state().tabs()[0].transaction);
    assert!(
        !h.get_by_role_and_label(Role::Button, "Save")
            .accesskit_node()
            .is_disabled()
    );
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "saved", |h| {
        h.state().tabs()[0].saves == 1 && !h.state().tabs()[0].unsaved()
    });
    assert_eq!(names_in(&path), ["Block", "sPin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_revert_after_dont_save_on_a_quit_still_closes_the_tab() {
    // Don't save on a quit while an edit is held: the revert that closes the tab waits
    // queued. Revert is off then; a revert asked all the same (as the disk banner's Reload
    // asks one) keeps the close, and the window closes.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    close_window(&mut h);
    step_until(&mut h, "the question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step(&mut h);
    assert!(h.state().tabs()[0].closing());
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    assert!(
        h.get_by_role_and_label(Role::Button, "Revert")
            .accesskit_node()
            .is_disabled()
    );
    let sessions = Arc::clone(h.state().sessions());
    h.state_mut().tabs_mut()[0].revert(&sessions, false);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn reload_is_off_while_the_tab_closes() {
    // Don't save on a close while an edit is held, then the file changes on disk: the
    // banner says so, and its Reload is off, the close dropping the edits already.
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step(&mut h);
    assert!(h.state().tabs()[0].closing());
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#),
    )
    .unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    assert!(
        h.get_by_role_and_label(Role::Button, "Reload")
            .accesskit_node()
            .is_disabled()
    );
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "closed", |h| h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_asterisk_follows_whether_the_transaction_changed_and_undo_and_redo_its_counts() {
    // status.transaction.changed (#300): an edit undone back to what `begin` saw leaves a
    // transaction open but nothing unsaved, so no asterisk, and the tab closes without
    // asking, nothing written. Undo and Redo are offered by the transaction's counts.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    assert!(!offered(&h, "Undo") && !offered(&h, "Redo"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the edit shown unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    step_until(&mut h, "Undo offered", |h| offered(h, "Undo"));
    assert!(!offered(&h, "Redo"));
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step_until(&mut h, "the undo shown saved", |h| {
        !anywhere(h, "example.silico *") && offered(h, "Redo")
    });
    assert!(h.state().tabs()[0].transaction);
    assert!(!offered(&h, "Undo"));
    h.get_by_role_and_label(Role::Button, "Redo").click();
    step_until(&mut h, "the redo shown unsaved", |h| {
        anywhere(h, "example.silico *") && offered(h, "Undo") && !offered(h, "Redo")
    });
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step_until(&mut h, "undone again", |h| !anywhere(h, "example.silico *"));
    // Closed with no question: rolled back, then closed.
    h.get_by_label("Close example.silico").click();
    step(&mut h);
    assert!(!anywhere(
        &h,
        "Save changes to example.silico before closing?"
    ));
    step_until(&mut h, "closed", |h| h.state().tabs().is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

/// Whether the app bar's button `word` is offered (enabled).
fn offered(h: &AppHarness, word: &str) -> bool {
    !h.get_by_role_and_label(Role::Button, word)
        .accesskit_node()
        .is_disabled()
}

/// Example open, Pin selected, "x" typed and answered, then undone back to the file's
/// document: a transaction open, nothing changed, Redo offered.
fn undone_back(f: &Fixture) -> (AppHarness, std::path::PathBuf) {
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the edit shown unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    step_until(&mut h, "Undo offered", |h| offered(h, "Undo"));
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step_until(&mut h, "undone back", |h| {
        !anywhere(h, "example.silico *") && offered(h, "Redo")
    });
    (h, path)
}

#[test]
fn a_close_while_a_redo_waits_asks_first() {
    // Undone back to the file's document, then Redo held behind the session's queue: the
    // tab holds an edit still to come back, so Close asks, and nothing is rolled back.
    let f = fixture();
    let (mut h, path) = undone_back(&f);
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_role_and_label(Role::Button, "Redo").click();
    step(&mut h);
    assert!(h.state().tabs()[0].unsaved());
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the redo shown", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "xPin")
    });
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_z_held_down_undoes_once_and_says_nothing() {
    // Two Ctrl+Z before the first answers (a key held down): Undo is off while a step is
    // under way, so the second asks nothing, and no "nothing to undo" is said.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the edit answered", |h| {
        h.state().tabs()[0].edits.idle() && offered(h, "Undo")
    });
    let before = h.state().tabs()[0].revision.number();
    let (hold, check) = hold_queue(&f, &h, 0);
    for _ in 0..2 {
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
        step(&mut h);
    }
    assert!(!offered(&h, "Undo"));
    hold.release();
    let _ = check.wait();
    // From the frame the undo's answer is taken, Undo is off: the one edit undone, before
    // and after a status of the new revision is read.
    let deadline = Instant::now() + WAIT;
    while !(!anywhere(&h, "example.silico *") && offered(&h, "Redo")) {
        step(&mut h);
        if h.state().tabs()[0].revision.number() > before {
            assert!(!offered(&h, "Undo"), "Undo offered with nothing to undo");
        }
        assert!(Instant::now() < deadline, "never undone");
    }
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].message, None);
    // One step: one revision.
    assert_eq!(h.state().tabs()[0].revision.number(), before + 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_refusal_said_is_cleared_by_the_next_action() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    empty_name(&mut h);
    assert!(h.state().tabs()[0].message.is_some());
    name_field(&h).type_text("B");
    step(&mut h);
    assert_eq!(h.state().tabs()[0].message, None);
    // Drawn so the next frame.
    step(&mut h);
    assert!(!anywhere(&h, "name is empty (give the feature a name)"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_y_redoes_and_redo_turns_off_once_typing_starts() {
    let f = fixture();
    let (mut h, _) = undone_back(&f);
    // Ctrl+Y: the edit back; from the frame its answer is taken, Redo is off.
    let before = h.state().tabs()[0].revision.number();
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Y);
    let deadline = Instant::now() + WAIT;
    while !(anywhere(&h, "example.silico *") && shows(&h, "Timeline", "xPin")) {
        step(&mut h);
        if h.state().tabs()[0].revision.number() > before {
            assert!(!offered(&h, "Redo"), "Redo offered with nothing to redo");
        }
        assert!(Instant::now() < deadline, "never redone");
    }
    // Undone again, Redo offered; then typing: Redo off at once (an edit waits), and off
    // after it answers (the edit dropped what was undone).
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step_until(&mut h, "undone again", |h| {
        !anywhere(h, "example.silico *") && offered(h, "Redo")
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    step(&mut h);
    assert!(!h.state().tabs()[0].edits.idle());
    assert!(!offered(&h, "Redo"));
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the typing answered", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    for _ in 0..10 {
        step(&mut h);
        assert!(!offered(&h, "Redo"));
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_file_changed_on_disk_under_a_transaction_that_changed_nothing_offers_no_choice() {
    // Undone back to the file's document: a transaction open, nothing unsaved. An agent's
    // change to the file isn't announced as one "while you have unsaved edits".
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = undone_back(&f);
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#),
    )
    .unwrap();
    step_until(&mut h, "the change found", |h| {
        h.state().tabs()[0]
            .status
            .as_ref()
            .is_some_and(|status| status.disk == Some(silico_core::disk::Disk::Changed))
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(!anywhere(
        &h,
        "example.silico changed on disk while you have unsaved edits."
    ));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_y_held_down_redoes_once_and_says_nothing() {
    let f = fixture();
    let (mut h, path) = undone_back(&f);
    let before = h.state().tabs()[0].revision.number();
    let (hold, check) = hold_queue(&f, &h, 0);
    for _ in 0..2 {
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Y);
        step(&mut h);
    }
    assert!(!offered(&h, "Redo"));
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "redone", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "xPin")
    });
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(h.state().tabs()[0].revision.number(), before + 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn file_close_is_off_with_no_document_open() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    assert!(h.get_by_label("Close").accesskit_node().is_disabled());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_tab_shown_is_the_one_selected_in_the_app_bar() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    let other = f.file("other.silico", EXAMPLE);
    h.state_mut().open(other.to_str().unwrap(), false);
    step_until(&mut h, "two tabs", |h| h.state().tabs().len() == 2);
    let selected = |h: &AppHarness, name: &str| {
        h.get_by_role_and_label(Role::Button, name)
            .accesskit_node()
            .toggled()
            .map(|toggled| toggled == eframe::egui::accesskit::Toggled::True)
    };
    step_until(&mut h, "the newest shown", |h| {
        selected(h, "other.silico") == Some(true)
    });
    assert_eq!(selected(&h, "example.silico"), Some(false));
    h.get_by_role_and_label(Role::Button, "example.silico")
        .click();
    step_until(&mut h, "the first shown", |h| {
        selected(h, "example.silico") == Some(true)
    });
    assert_eq!(selected(&h, "other.silico"), Some(false));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn open_with_no_path_opens_nothing_and_keeps_asking() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    button_off_start(&h, "Open…").click();
    step_until(&mut h, "the question", |h| anywhere(h, "Open document"));
    h.get_by_role_and_label(Role::Button, "Open")
        .click_accesskit();
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(anywhere(&h, "Open document"));
    assert!(h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_feature_selected_is_the_one_selected_in_the_timeline_and_the_browser() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    select(&mut h, "Pin");
    let selected = |h: &AppHarness, pane: &str, name: &str| {
        h.get_by_role_and_label(Role::Pane, pane)
            .get_by_label(name)
            .accesskit_node()
            .toggled()
            .map(|toggled| toggled == eframe::egui::accesskit::Toggled::True)
    };
    step_until(&mut h, "Pin selected", |h| {
        selected(h, "Timeline", "Pin") == Some(true)
    });
    assert_eq!(selected(&h, "Timeline", "Hole"), Some(false));
    assert_eq!(selected(&h, "Timeline", "Block"), Some(false));
    // The browser lists Hole, the one body, as selectable: not selected while Pin is.
    assert_eq!(selected(&h, "Browser", "Hole"), Some(false));
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Hole")
        .click();
    step_until(&mut h, "Hole selected", |h| {
        selected(h, "Browser", "Hole") == Some(true)
            && selected(h, "Timeline", "Hole") == Some(true)
    });
    assert_eq!(selected(&h, "Timeline", "Pin"), Some(false));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_name_past_the_basic_plane_is_saved_as_typed() {
    // A character past U+FFFF, which JSON writes as itself (no escape of four digits
    // could hold it): the edit is made, and the document holds it.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "\u{1F529}");
    step_until(&mut h, "the edit applied", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "\u{1F529}Pin")
    });
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains("\"name\": \"\u{1F529}Pin\""), "{text}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_quit_asked_while_a_save_waits_queued_keeps_that_save_and_closes() {
    // A keystroke in flight behind the session's held queue, then Ctrl+S (queued behind
    // it), then a quit's Save all: the save asked last takes the close too, so once both
    // edits are saved the window closes.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    let typed = name_field(&h).value().unwrap();
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step(&mut h);
    close_window(&mut h);
    step_until(&mut h, "the question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Save all")
        .click_accesskit();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until_event(&mut h, |h| told(h, true));
    assert_eq!(names_in(&path), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_one_letter_name_emptied_is_refused_and_the_field_keeps_it_empty() {
    // "P" emptied with one Delete: the only text typed is "", refused; the document's
    // name, "P", unchanged and never typed, so the field keeps "" because the name it
    // shows from is the one it started from.
    let f = fixture();
    let one = EXAMPLE.replace(r#""name": "Pin""#, r#""name": "P""#);
    let (mut h, _) = f.opened("example.silico", &one);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "P"));
    select(&mut h, "P");
    name_field(&h).focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    step_until(&mut h, "the empty name refused", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].message.is_some()
    });
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(name_field(&h).value().as_deref(), Some(""));
    assert_eq!(never_waits::waits(), 0);
}

// ---- Save As (docs/design/m5.0-app.md, section 4; #305's `commit` with a path) ----

/// The Save As dialog's path field.
fn path_field(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::TextInput, "Path")
}

/// File > Save As…, `path` typed into its dialog, and Save.
fn save_as(h: &mut AppHarness, path: &Path) {
    h.get_by_label("File").click();
    step(h);
    step(h);
    h.get_by_role_and_label(Role::Button, "Save As…").click();
    step_until(h, "the Save As dialog", |h| {
        h.query_by_label_contains("Save example.silico as")
            .is_some()
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
        h.query_by_label_contains("Save example.silico as")
            .is_none()
    });
}

#[test]
fn save_as_writes_the_new_file_and_the_tab_follows_it() {
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    let copy = f.dir.path().join("copy.silico");
    save_as(&mut h, &copy);
    step_until(&mut h, "the tab of the new file", |h| {
        anywhere(h, "copy.silico") && !anywhere(h, "copy.silico *")
    });
    assert_eq!(h.state().tabs()[0].path, copy.to_str().unwrap());
    // The new file holds the document as the session holds it, byte for byte; the old one
    // is as it was.
    let id = h.state().tabs()[0].id;
    let held = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), held);
    assert_eq!(names_in(&copy), ["Block", "xPin", "Hole"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    // A later save goes to the new file.
    type_name(&mut h, "y");
    step_until(&mut h, "unsaved again", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "copy.silico *")
    });
    let typed = name_field(&h).value().unwrap();
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "saved", |h| !anywhere(h, "copy.silico *"));
    assert_eq!(names_in(&copy), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_saves_only_on_its_save_and_only_with_a_path() {
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Save As…").click();
    step_until(&mut h, "the Save As dialog", |h| {
        anywhere(h, "Save example.silico as")
    });
    // A path typed, no click: nothing saved, however many frames go by.
    let copy = f.dir.path().join("copy.silico");
    path_field(&h).focus();
    step(&mut h);
    path_field(&h).type_text(copy.to_str().unwrap());
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(anywhere(&h, "Save example.silico as"));
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert!(!copy.exists());
    // The path cleared, then Save clicked: nothing either, and the dialog stays.
    // Focused again (its cursor at its start), each character deleted.
    path_field(&h).focus();
    step(&mut h);
    for _ in 0..copy.to_str().unwrap().chars().count() {
        h.key_press(eframe::egui::Key::Delete);
        step(&mut h);
    }
    assert_eq!(path_field(&h).value().as_deref(), Some(""));
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(anywhere(&h, "Save example.silico as"));
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert_eq!(h.state().tabs()[0].path, old.to_str().unwrap());
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_shift_s_asks_where_and_saves_nothing_until_told() {
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    h.key_press_modifiers(
        eframe::egui::Modifiers::COMMAND | eframe::egui::Modifiers::SHIFT,
        eframe::egui::Key::S,
    );
    step_until(&mut h, "the Save As dialog", |h| {
        anywhere(h, "Save example.silico as")
    });
    for _ in 0..5 {
        step(&mut h);
    }
    // Not Ctrl+S: nothing saved.
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert!(anywhere(&h, "example.silico *"));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_over_a_file_asks_and_cancel_writes_nothing_and_replace_writes() {
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    let there = f.file("there.silico", "an older copy");
    save_as(&mut h, &there);
    let asked = format!("{} already exists. Replace it?", there.to_str().unwrap());
    step_until(&mut h, "the question", |h| anywhere(h, &asked));
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the question gone", |h| !anywhere(h, &asked));
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(std::fs::read_to_string(&there).unwrap(), "an older copy");
    assert!(anywhere(&h, "example.silico *"));
    save_as(&mut h, &there);
    step_until(&mut h, "the question again", |h| anywhere(h, &asked));
    h.get_by_role_and_label(Role::Button, "Replace")
        .click_accesskit();
    step_until(&mut h, "the tab of the file replaced", |h| {
        anywhere(h, "there.silico") && !anywhere(h, "there.silico *")
    });
    assert_eq!(names_in(&there), ["Block", "xPin", "Hole"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_as_the_core_refuses_is_said_and_the_tab_stays_on_its_file() {
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    let json = f.dir.path().join("copy.json");
    save_as(&mut h, &json);
    step_until(&mut h, "the refusal said", |h| {
        h.query_by_label_contains("doesn't end in .silico")
            .is_some()
    });
    assert!(!json.exists());
    assert_eq!(h.state().tabs()[0].path, old.to_str().unwrap());
    assert!(anywhere(&h, "example.silico *"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_takes_the_edits_made_before_it_and_one_with_none_open_writes_all_the_same() {
    // A keystroke held behind the session's queue, then Save As: the copy has it.
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    let typed = name_field(&h).value().unwrap();
    let copy = f.dir.path().join("copy.silico");
    save_as(&mut h, &copy);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the tab of the new file", |h| {
        anywhere(h, "copy.silico") && !anywhere(h, "copy.silico *")
    });
    assert_eq!(names_in(&copy), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    // Then Save As with nothing changed, no transaction open: written all the same.
    let again = f.dir.path().join("again.silico");
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Save As…").click();
    step_until(&mut h, "the dialog", |h| anywhere(h, "Save copy.silico as"));
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text(again.to_str().unwrap());
    step(&mut h);
    // The dialog's, after the app bar's.
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "the tab of the third file", |h| {
        anywhere(h, "again.silico") && !anywhere(h, "again.silico *")
    });
    assert_eq!(names_in(&again), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn typing_while_a_save_as_runs_comes_after_it_in_a_new_transaction() {
    // The Save As held behind the session's queue, then a keystroke: sent after the Save
    // As has answered, so it begins a transaction on the new file, unsaved, never written
    // to it as an edit outside one.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    let copy = f.dir.path().join("copy.silico");
    save_as(&mut h, &copy);
    step_until(&mut h, "the Save As sent", |h| {
        h.state().tabs()[0].saves == 1
    });
    type_name(&mut h, "z");
    let typed = name_field(&h).value().unwrap();
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the keystroke applied", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", &typed)
    });
    step_until(&mut h, "unsaved on the new file", |h| {
        anywhere(h, "copy.silico *")
    });
    assert!(h.state().tabs()[0].transaction);
    assert_eq!(names_in(&copy), ["Block", "xPin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_refused_save_as_that_began_its_own_transaction_rolls_it_back() {
    // No transaction open, then Save As to a file already there: the Save As began one,
    // and, asked whether to replace and told Cancel, the tab holds none, so an agent's
    // change to the file is reloaded as with no Save As tried.
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let there = f.file("there.silico", "an older copy");
    save_as(&mut h, &there);
    let asked = format!("{} already exists. Replace it?", there.to_str().unwrap());
    step_until(&mut h, "the question", |h| anywhere(h, &asked));
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "rolled back", |h| {
        !anywhere(h, &asked) && !h.state().tabs()[0].transaction && h.state().tabs()[0].edits.idle()
    });
    assert_eq!(std::fs::read_to_string(&there).unwrap(), "an older copy");
    std::fs::write(
        &old,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#),
    )
    .unwrap();
    step_until(&mut h, "the agent's change reloaded", |h| {
        shows(h, "Timeline", "Drill")
    });
    assert!(!anywhere(&h, "example.silico *"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_takes_every_edit_before_it_and_none_after() {
    // Two keystrokes before it (one in flight, one queued) and one after, all held behind
    // the session's queue: the new file has the two, and the third is a new transaction's.
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "1");
    type_name(&mut h, "2");
    let before = name_field(&h).value().unwrap();
    let copy = f.dir.path().join("copy.silico");
    save_as(&mut h, &copy);
    type_name(&mut h, "3");
    let after = name_field(&h).value().unwrap();
    assert_ne!(before, after);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the last keystroke applied", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", &after)
    });
    step_until(&mut h, "unsaved on the new file", |h| {
        anywhere(h, "copy.silico *")
    });
    assert_eq!(names_in(&copy), ["Block", before.as_str(), "Hole"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_as_queued_behind_a_save_begins_again_and_writes() {
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    let copy = f.dir.path().join("copy.silico");
    save_as(&mut h, &copy);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the tab of the new file", |h| {
        anywhere(h, "copy.silico") && !anywhere(h, "copy.silico *")
    });
    assert_eq!(names_in(&old), ["Block", "xPin", "Hole"]);
    assert_eq!(names_in(&copy), ["Block", "xPin", "Hole"]);
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_revert_after_a_queued_save_as_keeps_it_and_has_nothing_left_to_revert() {
    let f = fixture();
    let (mut h, old) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    let typed = name_field(&h).value().unwrap();
    let copy = f.dir.path().join("copy.silico");
    save_as(&mut h, &copy);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the tab of the new file", |h| {
        anywhere(h, "copy.silico") && !anywhere(h, "copy.silico *")
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(names_in(&copy), ["Block", typed.as_str(), "Hole"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_replace_question_waits_for_another_question_to_be_answered() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    let there = f.file("there.silico", "an older copy");
    let (hold, check) = hold_queue(&f, &h, 0);
    save_as(&mut h, &there);
    // Another question asked while the Save As waits.
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "New…").click();
    step_until(&mut h, "the New question", |h| anywhere(h, "New document"));
    hold.release();
    let _ = check.wait();
    let asked = format!("{} already exists. Replace it?", there.to_str().unwrap());
    for _ in 0..20 {
        step(&mut h);
        assert!(
            anywhere(&h, "New document"),
            "the New question was replaced"
        );
    }
    assert!(!anywhere(&h, &asked));
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the Replace question", |h| anywhere(h, &asked));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_is_off_while_the_tab_closes() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    h.get_by_role_and_label(Role::Button, "Don't save")
        .click_accesskit();
    step(&mut h);
    assert!(h.state().tabs()[0].closing());
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    assert!(
        h.get_by_role_and_label(Role::Button, "Save As…")
            .accesskit_node()
            .is_disabled()
    );
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    h.key_press_modifiers(
        eframe::egui::Modifiers::COMMAND | eframe::egui::Modifiers::SHIFT,
        eframe::egui::Key::S,
    );
    step(&mut h);
    assert!(!anywhere(&h, "Save example.silico as"));
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "closed", |h| h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

// ---- Feature dialogs (M5.8; section 9, *Feature dialogs*) ----

/// A washer and a plate, and a triangle sketched on XY (the M4.5 measures' padded
/// triangle's sketch), not yet swept: the plate, made last, is the tip.
const SKETCHED: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000002", "type": "box", "v": 1, "name": "Washer", "position": [50, 0, 0], "length": 5, "width": 5, "height": 1},
    {"id": "b0000001", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, -10], "length": 40, "width": 40, "height": 10},
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 20, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

/// The open dialog.
fn dialog(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role(Role::Dialog)
}

/// Whether a dialog is open.
fn dialog_open(h: &AppHarness) -> bool {
    h.query_by_role(Role::Dialog).is_some()
}

/// The dialog of `kind` opened from the toolbar's CREATE.
fn create(h: &mut AppHarness, kind: &str) {
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label(kind)
        .click();
    step_until(h, "the dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, kind).is_some()
    });
}

/// The dialog's input labelled `label`.
fn input<'a>(h: &'a AppHarness, label: &'a str) -> egui_kittest::Node<'a> {
    dialog(h).get_by_role_and_label(Role::TextInput, label)
}

/// Typed into the dialog's input `label`, one keystroke a frame, at its start.
fn type_into(h: &mut AppHarness, label: &str, text: &str) {
    input(h, label).focus();
    step(h);
    for c in text.chars() {
        input(h, label).type_text(&c.to_string());
        step(h);
    }
}

/// `pane`'s node labelled `label` double-clicked: both clicks in one frame, as the
/// harness's own clicks each take frames of their own, a quarter second apart, which egui
/// counts as two single clicks.
fn double_click(h: &mut AppHarness, pane: &str, label: &str) {
    use eframe::egui::{Event, Modifiers, PointerButton};
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

/// Whether the dialog's OK is on.
fn ok_enabled(h: &AppHarness) -> bool {
    !dialog(h)
        .get_by_role_and_label(Role::Button, "OK")
        .accesskit_node()
        .is_disabled()
}

/// The session's document text, as `text` gives it.
fn text_of(f: &Fixture, h: &AppHarness) -> String {
    let id = h.state().tabs()[0].id;
    f.sessions.text(&id, None).wait().result.unwrap().text
}

/// The fields of the feature named `name` in `text`, but its `id`, `v`, and `fingerprint`,
/// each as JSON writes it: none if no feature has that name.
fn fields_of(text: &str, name: &str) -> Option<Vec<(String, Plain)>> {
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
fn sorted_fields(fields: Vec<(String, Plain)>) -> Vec<(String, Plain)> {
    let mut fields: Vec<(String, Plain)> = fields
        .into_iter()
        .filter(|(key, _)| !["id", "v", "fingerprint"].contains(&key.as_str()))
        .collect();
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    fields
}

/// The fields `json` writes, as [`fields_of`] gives them.
fn expected(json_text: &str) -> Vec<(String, Plain)> {
    let Plain::Object(fields) = Plain::of(&json::read(json_text.as_bytes()).unwrap()) else {
        panic!("not an object");
    };
    sorted_fields(fields)
}

/// Stepped until nothing of the tab's is under way and its last answer's revision is read.
fn settled(h: &mut AppHarness) {
    step_until(h, "everything answered", |h| {
        let tab = &h.state().tabs()[0];
        tab.edits.idle() && !tab.unsettled() && tab.dialog.as_ref().is_none_or(|d| !d.adding)
    });
}

#[test]
fn each_kinds_dialog_adds_its_feature_with_its_defaults_on_ok() {
    // The defaults are the dialog's (the M5.8 brief: a 10 mm cube, a cylinder of radius 5
    // and height 10, ...), each written here; a sweep takes the sketch selected and the
    // body last on the timeline as its base.
    let cases: [(&str, &str); 9] = [
        (
            "Box",
            r#"{"type": "box", "name": "Box", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        ),
        (
            "Cylinder",
            r#"{"type": "cylinder", "name": "Cylinder", "position": [0, 0, 0], "radius": 5, "height": 10}"#,
        ),
        (
            "Sphere",
            r#"{"type": "sphere", "name": "Sphere", "position": [0, 0, 0], "radius": 5}"#,
        ),
        (
            "Cone",
            r#"{"type": "cone", "name": "Cone", "position": [0, 0, 0], "radius1": 5, "radius2": 0, "height": 10}"#,
        ),
        (
            "Torus",
            r#"{"type": "torus", "name": "Torus", "position": [0, 0, 0], "radius1": 10, "radius2": 2}"#,
        ),
        (
            "Pad",
            r#"{"type": "pad", "name": "Pad", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}"#,
        ),
        (
            "Pocket",
            r#"{"type": "pocket", "name": "Pocket", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#,
        ),
        (
            "Revolve",
            r#"{"type": "revolve", "name": "Revolve", "base": "b0000001", "sketch": "s0000001", "axis": "y_axis", "extent": {"type": "one_side", "angle": 360, "reversed": false}}"#,
        ),
        (
            "Groove",
            r#"{"type": "groove", "name": "Groove", "base": "b0000001", "sketch": "s0000001", "axis": "y_axis", "extent": {"type": "one_side", "angle": 360, "reversed": false}}"#,
        ),
    ];
    for (kind, want) in cases {
        let f = fixture();
        let (mut h, _) = f.opened("sketched.silico", SKETCHED);
        step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
        h.get_by_role_and_label(Role::Pane, "Browser")
            .get_by_label("Triangle")
            .click();
        step(&mut h);
        create(&mut h, kind);
        step_until(&mut h, "its add answered", |h| {
            h.state().tabs()[0]
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.feature.is_some())
        });
        assert!(ok_enabled(&h), "{kind}");
        dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
        step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
        settled(&mut h);
        let text = text_of(&f, &h);
        assert_eq!(fields_of(&text, kind), Some(expected(want)), "{kind}");
        // The earlier features as they were.
        assert_eq!(
            fields_of(&text, "Plate"),
            fields_of(SKETCHED, "Plate"),
            "{kind}"
        );
        assert_eq!(
            h.state().tabs()[0].sent[..2],
            [Sent::Begin, Sent::Add],
            "{kind}"
        );
        assert_eq!(never_waits::waits(), 0);
    }
}

#[test]
fn a_sweeps_ok_waits_for_a_sketch_chosen_in_the_browser_and_adds_nothing_before() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    // A body selected, not a sketch: the Pad's selection box is empty, OK off, nothing
    // added; nor does a body chosen in the browser fill it.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Washer")
        .click();
    step(&mut h);
    create(&mut h, "Pad");
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Plate")
        .click();
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(!ok_enabled(&h));
    assert!(dialog(&h).query_by_label("Select in the browser").is_some());
    assert!(h.state().tabs()[0].sent.is_empty());
    // The sketch chosen in the browser fills it: added, OK on.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    assert!(dialog(&h).query_by_label("Triangle").is_some());
    assert!(ok_enabled(&h));
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(
        fields_of(&text_of(&f, &h), "Pad"),
        Some(expected(
            r#"{"type": "pad", "name": "Pad", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}"#
        ))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_pockets_extent_form_shows_its_own_inputs_and_writes_only_its_own_values() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step(&mut h);
    create(&mut h, "Pocket");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    assert!(
        dialog(&h)
            .query_by_role_and_label(Role::TextInput, "Length 2")
            .is_none()
    );
    dialog(&h).get_by_label("Two sides").click();
    step_until(&mut h, "two sides", |h| {
        dialog(h)
            .query_by_role_and_label(Role::TextInput, "Length 2")
            .is_some()
    });
    assert!(dialog(&h).query_by_label("Reversed").is_none());
    type_into(&mut h, "Length 2", "1");
    settled(&mut h);
    let pocket = fields_of(&text_of(&f, &h), "Pocket").unwrap();
    let extent = pocket.iter().find(|(key, _)| key == "extent").unwrap();
    assert_eq!(
        extent.1,
        Plain::of(&json::read(br#"{"type": "two_sides", "length": 5, "length2": 15}"#).unwrap())
    );
    dialog(&h).get_by_label("Through all").click();
    step_until(&mut h, "through all", |h| {
        dialog(h)
            .query_by_role_and_label(Role::TextInput, "Length")
            .is_none()
    });
    assert!(dialog(&h).query_by_label("Reversed").is_some());
    settled(&mut h);
    let pocket = fields_of(&text_of(&f, &h), "Pocket").unwrap();
    let extent = pocket.iter().find(|(key, _)| key == "extent").unwrap();
    assert_eq!(
        extent.1,
        Plain::of(&json::read(br#"{"type": "through_all", "reversed": false}"#).unwrap())
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_dialogs_value_typed_is_coalesced_into_one_edit_behind_the_one_in_flight() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    settled(&mut h);
    let sent = h.state().tabs()[0].edits.sent;
    // The session's queue held: the first keystroke's edit waits in it, and the next two
    // wait in the tab, the last replacing the one before.
    let (hold, check) = hold_queue(&f, &h, 0);
    type_into(&mut h, "Length", "123");
    hold.release();
    check.wait();
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].edits.sent, sent + 2);
    assert_eq!(
        h.state().tabs()[0].sent[h.state().tabs()[0].sent.len() - 2..],
        [
            Sent::Edit("length".to_owned()),
            Sent::Edit("length".to_owned())
        ]
    );
    // The field's text, three digits and the 10 (the harness places each keystroke's
    // cursor as it does), is the document's.
    let typed = input(&h, "Length").value().unwrap();
    assert_eq!(typed.len(), 5);
    let text = text_of(&f, &h);
    let fields = fields_of(&text, "Box").unwrap();
    assert!(
        fields.contains(&("length".to_owned(), Plain::Number(typed.clone()))),
        "{typed}: {fields:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_unparseable_value_is_red_sends_nothing_and_keeps_ok_off() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Sphere");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    settled(&mut h);
    let sent = h.state().tabs()[0].edits.sent;
    type_into(&mut h, "Radius", "x");
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("x5"));
    assert!(!ok_enabled(&h));
    assert_eq!(h.state().tabs()[0].edits.sent, sent);
    // Enter is OK only when OK is on.
    h.key_press(eframe::egui::Key::Enter);
    step(&mut h);
    assert!(dialog_open(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_value_the_core_refuses_keeps_ok_off_until_one_it_takes() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Sphere");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    settled(&mut h);
    // "-5": a number, sent, and refused by the gate (a radius is above 0).
    type_into(&mut h, "Radius", "-");
    settled(&mut h);
    // Said in the dialog, as the core words it.
    let said = h.state().tabs()[0].message.clone().unwrap();
    assert!(dialog(&h).query_by_label(&said).is_some(), "{said}");
    assert!(!ok_enabled(&h));
    // "5" again: taken, OK on.
    h.key_press(eframe::egui::Key::Home);
    step(&mut h);
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("5"));
    assert!(ok_enabled(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_undoes_to_where_the_dialog_opened_byte_for_byte_keeping_earlier_edits() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    // An edit before the dialog, in the transaction.
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    settled(&mut h);
    let before = text_of(&f, &h);
    assert!(before.contains(r#""name": "KPin""#));
    let opened_at = h.state().tabs()[0].revision;
    create(&mut h, "Box");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    type_into(&mut h, "Height", "2");
    settled(&mut h);
    assert!(text_of(&f, &h).contains(r#""height": 210"#));
    dialog(&h)
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    let tab = &h.state().tabs()[0];
    assert!(tab.transaction);
    assert_eq!(tab.sent.last(), Some(&Sent::Undo(Some(opened_at))));
    assert!(!tab.sent.contains(&Sent::Rollback));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_failed_preview_shows_the_cores_error_in_the_dialog_and_ok_keeps_the_feature() {
    let f = fixture();
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cone { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "the cone came apart"))
    });
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Cone");
    step_until(&mut h, "the error in the dialog", |h| {
        dialog(h)
            .query_all_by_label_contains("the cone came apart")
            .next()
            .is_some()
    });
    assert!(ok_enabled(&h));
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    let text = text_of(&f, &h);
    assert_eq!(
        fields_of(&text, "Cone"),
        Some(expected(
            r#"{"type": "cone", "name": "Cone", "position": [0, 0, 0], "radius1": 5, "radius2": 0, "height": 10}"#
        ))
    );
    assert!(
        !h.state().tabs()[0]
            .sent
            .iter()
            .any(|sent| matches!(sent, Sent::Undo(_) | Sent::Rollback))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_stops_a_dialogs_preview_rebuild_first_and_cancels_the_dialog_second() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let before = text_of(&f, &h);
    // The new box's build held: its preview's rebuild runs until released.
    let hold = f.kernel.hold(is_box);
    create(&mut h, "Box");
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    // A refusal said before: the stop, succeeding, clears it.
    h.state_mut().tabs_mut()[0].message = Some("an earlier refusal".to_owned());
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the rebuild stopped", |h| {
        h.state().tabs()[0].status.as_ref().is_some_and(|status| {
            status.rebuild.state == silico_core::command::RebuildState::Stopped
        })
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(dialog_open(&h));
    assert!(!h.state().tabs()[0].preview_rebuilding());
    assert_eq!(h.state().tabs()[0].message, None);
    assert!(!h.state().tabs()[0].sent.contains(&Sent::Undo(None)));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    // Its own add began the transaction: rolled back, nothing before it to keep.
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Rollback));
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

/// On this thread, outside any frame, until `done` holds, within [`WAIT`]: a session's or
/// the kernel's state, which no frame moves.
#[track_caller]
fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "never {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The box a dialog's defaults make, 10 mm wide: narrower than the example's block.
fn is_new_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx < 20.0)
}

#[test]
fn esc_stops_a_previews_rebuild_for_good_though_its_read_still_waits_in_the_queue() {
    // The preview's `timeline` read queued behind another command that waits on the same
    // rebuild (a check of the current model), so it isn't the rebuild's waiter yet when Esc
    // stops it. The next command that needs the model starts a stopped rebuild again
    // (decision 34): run after the stop, the read would, undoing it. The stop holds: the
    // read answers `cancelled`, the tab reads the published model, and the new box is asked
    // of the kernel once.
    use silico_core::command::{RebuildState, RunningCommand, SessionCommand};
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let before = text_of(&f, &h);
    let id = h.state().tabs()[0].id;
    let box_hold = f.kernel.hold(is_new_box);
    // The queue held behind a fresh check whose pin the fake holds: the dialog's begin and
    // add are queued behind it.
    let pin_hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. })));
    let fresh = f.sessions.check(&id, CheckModel::Current { fresh: true });
    create(&mut h, "Box");
    step_until(&mut h, "its add sent", |h| {
        h.state().tabs()[0].sent.contains(&Sent::Add)
    });
    // No frame from here until the check below is queued: the app takes no answer, so the
    // preview's read can only be sent after it.
    pin_hold.release();
    assert!(fresh.wait_timeout(WAIT).ok().unwrap().result.is_ok());
    // Queued behind the add: it needs the new revision's model, whose rebuild it starts and
    // the fake holds at the new box.
    let waiting = f.sessions.check(&id, CheckModel::Current { fresh: false });
    assert!(f.kernel.wait_running(1, WAIT));
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    // Sent with the add's answer taken, its read waits in the queue behind the check.
    let added = h.state().tabs()[0].revision;
    assert_eq!(
        f.sessions.status(&id).result.unwrap().running,
        Some(RunningCommand::Command(SessionCommand::Check))
    );
    h.state_mut().tabs_mut()[0].message = Some("an earlier refusal".to_owned());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    // The check, the rebuild's waiter, answers `cancelled` with it.
    let answered = waiting.wait_timeout(WAIT).ok().unwrap();
    assert_eq!(
        answered.result.unwrap_err().errors()[0].code(),
        ErrorCode::Cancelled
    );
    let new_boxes = || {
        f.kernel
            .asked()
            .iter()
            .filter(|call| is_new_box(&call.op))
            .count()
    };
    // Then the read, run (it starts the rebuild again, asking for the new box) or answered
    // `cancelled` (the tab then reads the published model, of the add's revision).
    step_until(&mut h, "the preview's read run or answered", |h| {
        new_boxes() > 1
            || h.state().tabs()[0]
                .timeline
                .as_ref()
                .is_some_and(|timeline| timeline.revision == added)
    });
    assert_eq!(new_boxes(), 1, "the stopped rebuild started again");
    let tab = &h.state().tabs()[0];
    assert!(tab.status.as_ref().is_some_and(|status| {
        status.rebuild.revision == added && status.rebuild.state == RebuildState::Stopped
    }));
    assert!(!tab.preview_rebuilding());
    assert!(dialog_open(&h));
    assert_eq!(tab.message, None);
    // The next Esc is the dialog's Cancel.
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(new_boxes(), 1);
    box_hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_stops_a_previews_rebuild_for_good_though_a_check_of_the_current_model_waits_in_the_queue() {
    // A check of the current model (the tab's while its timeline names no model, a marked
    // feature selected) queued behind the preview's read, the rebuild's waiter. Run after
    // the stop, it would start the rebuild again (decision 34). The stop holds: the check
    // answers `cancelled`, none of the current model is asked again while the rebuild is
    // stopped, and the new box is asked of the kernel once.
    use silico_core::command::{RebuildState, RunningCommand, SessionCommand};
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let before = text_of(&f, &h);
    let id = h.state().tabs()[0].id;
    let hold = f.kernel.hold(is_new_box);
    create(&mut h, "Box");
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    let added = h.state().tabs()[0].revision;
    until("the preview's read waiting", || {
        f.sessions.status(&id).result.is_ok_and(|status| {
            status.running == Some(RunningCommand::Command(SessionCommand::Timeline))
        })
    });
    // Its timeline as one naming no model gives it, a marked feature selected: the next
    // frame sends a check of the current model, queued behind the read.
    {
        let tab = &mut h.state_mut().tabs_mut()[0];
        assert!(!tab.checking());
        let timeline = tab.timeline.as_mut().unwrap();
        timeline.model = None;
        let marked = timeline
            .features
            .iter()
            .flatten()
            .find(|entry| entry.problems > 0 || entry.warnings > 0)
            .map(|entry| entry.id.clone());
        assert!(marked.is_some(), "no feature of the example is marked");
        tab.selected = marked;
    }
    step_until(&mut h, "the check sent", |h| h.state().tabs()[0].checking());
    // Queued: the read still waits on the rebuild, which the fake holds at the new box. The
    // stop cancels the check, sent later, before the read: the read, cancelled, stops the
    // rebuild and answers, and its session runs the check next, which, cancelled second,
    // could start the rebuild again first (the tab's unit test
    // `the_stop_cancels_the_tabs_read_sent_later_first_so_neither_starts_the_rebuild_again`
    // forces that).
    assert_eq!(
        f.sessions.status(&id).result.unwrap().running,
        Some(RunningCommand::Command(SessionCommand::Timeline))
    );
    h.state_mut().tabs_mut()[0].message = Some("an earlier refusal".to_owned());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    let new_boxes = || {
        f.kernel
            .asked()
            .iter()
            .filter(|call| is_new_box(&call.op))
            .count()
    };
    // Then the check, run (it starts the rebuild again, asking for the new box) or answered
    // `cancelled`, and the tab's timeline read again, of the published model.
    step_until(&mut h, "the check run or answered", |h| {
        let tab = &h.state().tabs()[0];
        new_boxes() > 1
            || (!tab.checking()
                && tab
                    .timeline
                    .as_ref()
                    .is_some_and(|timeline| timeline.revision == added))
    });
    assert_eq!(new_boxes(), 1, "the stopped rebuild started again");
    let tab = &h.state().tabs()[0];
    assert!(tab.status.as_ref().is_some_and(|status| {
        status.rebuild.revision == added && status.rebuild.state == RebuildState::Stopped
    }));
    assert!(!tab.preview_rebuilding());
    assert!(dialog_open(&h));
    assert_eq!(tab.message, None);
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(new_boxes(), 1);
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_and_an_esc_in_one_frame_supersede_the_preview_then_cancel_the_dialog() {
    // A keystroke and Esc in one pass of the frame, the preview rebuilding, as the frame
    // takes them: the dialog's changes sent (`dialog_changed`), then its key
    // (`dialog_escape`), on the `status` the pass's poll read. The edit supersedes the
    // preview's read, which stops its rebuild (of a revision the edit makes old). That stop
    // is the edit's: the Esc has no preview of its own to stop, and is the dialog's Cancel,
    // never eaten as a second stop. (Driven on the tab: a keystroke and Esc given to one
    // harness step reach the Esc's key with `status` already at the edit's revision, the
    // edit sent in an earlier pass of the step, so the two never meet in one pass there.)
    use silico_app::dialog::Value;
    use silico_core::command::{RunningCommand, SessionCommand};
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let before = text_of(&f, &h);
    let id = h.state().tabs()[0].id;
    let hold = f.kernel.hold(is_new_box);
    create(&mut h, "Box");
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    until("the preview's read waiting", || {
        f.sessions.status(&id).result.is_ok_and(|status| {
            status.running == Some(RunningCommand::Command(SessionCommand::Timeline))
        })
    });
    let tab = &mut h.state_mut().tabs_mut()[0];
    tab.message = Some("an earlier refusal".to_owned());
    // "3" typed before the width's 10.
    let dialog = tab.dialog.as_mut().unwrap();
    let width = dialog
        .inputs
        .iter_mut()
        .find(|input| input.label == "Width")
        .unwrap();
    width.value = Value::Length("310".to_owned());
    assert!(tab.dialog_changed(&f.sessions));
    assert!(
        tab.sent
            .iter()
            .any(|sent| matches!(sent, Sent::Edit(field) if field == "width")),
        "{:?}",
        tab.sent
    );
    // The pass's `status`, read before the edit, still says the preview rebuilds.
    assert!(tab.preview_rebuilding());
    assert!(
        tab.dialog_escape(&f.sessions),
        "the Esc was taken as a stop"
    );
    tab.close_dialog(&f.sessions, false);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Rollback));
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_in_a_dialog_whose_preview_was_built_since_status_said_it_rebuilt_is_its_cancel() {
    // `status`, read at the frame's start, says the preview rebuilds; its rebuild publishes
    // before Esc's stop reaches it. The stop finds nothing to stop: the Esc is the dialog's
    // Cancel, never eaten, and nothing is said.
    use silico_core::command::RebuildState;
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let id = h.state().tabs()[0].id;
    let hold = f.kernel.hold(is_new_box);
    create(&mut h, "Box");
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    // Built, with no frame since: the tab's `status` still says it rebuilds.
    hold.release();
    until("the preview built", || {
        f.sessions
            .status(&id)
            .result
            .is_ok_and(|status| status.rebuild.state == RebuildState::Done)
    });
    let tab = &mut h.state_mut().tabs_mut()[0];
    assert!(tab.preview_rebuilding());
    tab.message = Some("an earlier refusal".to_owned());
    assert!(tab.dialog_escape(&f.sessions));
    assert_eq!(tab.message.as_deref(), Some("an earlier refusal"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_whose_previews_read_stopped_the_rebuild_first_stops_it_leaving_the_dialog_open() {
    // The preview's read, cancelled by Esc's stop, wakes and stops the rebuild it waits on
    // before the cancel naming the rebuild reaches it, which is then refused: the rebuild
    // stopped all the same, so Esc stopped it, the dialog left open, the earlier refusal
    // let go.
    use silico_core::command::{Cancellable, RebuildState, RunningCommand, SessionCommand};
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let id = h.state().tabs()[0].id;
    let hold = f.kernel.hold(is_new_box);
    create(&mut h, "Box");
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    until("the preview's read waiting", || {
        f.sessions.status(&id).result.is_ok_and(|status| {
            status.running == Some(RunningCommand::Command(SessionCommand::Timeline))
        })
    });
    // Its read cancelled (the ticket its handle shares), which stops the rebuild, with no
    // frame since: the tab's `status` still says it rebuilds.
    f.sessions
        .cancel(&id, Cancellable::Command(SessionCommand::Timeline))
        .result
        .unwrap();
    until("the rebuild stopped by its read", || {
        f.sessions
            .status(&id)
            .result
            .is_ok_and(|status| status.rebuild.state == RebuildState::Stopped)
    });
    let tab = &mut h.state_mut().tabs_mut()[0];
    assert!(tab.preview_rebuilding());
    tab.message = Some("an earlier refusal".to_owned());
    assert!(!tab.dialog_escape(&f.sessions));
    assert_eq!(tab.message, None);
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_value_typed_while_the_preview_rebuilds_is_applied_without_waiting_for_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let hold = f.kernel.hold(is_box);
    create(&mut h, "Box");
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    // The rebuild still held: the edit lands all the same, the old preview's read stopped.
    type_into(&mut h, "Width", "3");
    step_until(&mut h, "the width applied", |h| {
        h.state().tabs()[0].edits.idle() && !h.state().tabs()[0].unsettled()
    });
    // Answered with the rebuild still held (the next preview's read waits for it, and a
    // read of the text would wait behind that).
    assert!(f.kernel.wait_running(1, WAIT));
    hold.release();
    let text = text_of(&f, &h);
    assert!(text.contains(r#""width": 310"#), "{text}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn enter_is_ok_and_esc_is_cancel() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let before = text_of(&f, &h);
    create(&mut h, "Torus");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    settled(&mut h);
    // One dialog at a time: CREATE's tools off.
    assert!(
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label("Box")
            .accesskit_node()
            .is_disabled()
    );
    // Undo and redo are the dialog's Cancel's while it's open: off.
    let sent = h.state().tabs()[0].sent.clone();
    assert!(
        h.get_by_role_and_label(Role::Button, "Undo")
            .accesskit_node()
            .is_disabled()
    );
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sent, sent);
    // Its preview built, as `status` says: with it still rebuilding, Esc would stop it,
    // leaving the dialog open (esc_stops_a_dialogs_preview_rebuild_first_and_cancels_the_
    // dialog_second).
    step_until(&mut h, "the preview built", |h| {
        let tab = &h.state().tabs()[0];
        tab.status.as_ref().is_some_and(|status| {
            status.rebuild.revision == tab.revision
                && status.rebuild.state == silico_core::command::RebuildState::Done
        })
    });
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    create(&mut h, "Torus");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert!(fields_of(&text_of(&f, &h), "Torus").is_some());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn after_ok_one_undo_takes_back_the_dialogs_edits_and_one_redo_gives_them_back() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    settled(&mut h);
    let before = text_of(&f, &h);
    let opened_at = h.state().tabs()[0].revision;
    create(&mut h, "Cylinder");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    // Two edits, each its own step on the stack: the add, the radius, the height.
    type_into(&mut h, "Radius", "1");
    settled(&mut h);
    type_into(&mut h, "Height", "2");
    settled(&mut h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    let after = text_of(&f, &h);
    let ok_at = h.state().tabs()[0].revision;
    assert!(after.contains(r#""radius": 15"#) && after.contains(r#""height": 210"#));
    step_until(&mut h, "undoable", |h| h.state().tabs()[0].undoable());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(
        h.state().tabs()[0].sent.last(),
        Some(&Sent::Undo(Some(opened_at)))
    );
    step_until(&mut h, "redoable", |h| h.state().tabs()[0].redoable());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Y);
    step(&mut h);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), after);
    assert_eq!(
        h.state().tabs()[0].sent.last(),
        Some(&Sent::Redo(Some(ok_at)))
    );
    // And once more: the group's ends include the redo's revision.
    step_until(&mut h, "undoable", |h| h.state().tabs()[0].undoable());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    // KPin, made before the dialog, stays: one more undo takes it back alone.
    assert!(before.contains(r#""name": "KPin""#));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn edit_feature_from_a_double_click_loads_the_features_values_and_ok_applies_a_change() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    // The Pin's values, as EXAMPLE writes them.
    for (label, value) in [
        ("X", "20"),
        ("Y", "10"),
        ("Z", "-1"),
        ("Radius", "2.5"),
        ("Height", "12"),
    ] {
        assert_eq!(input(&h, label).value().as_deref(), Some(value), "{label}");
    }
    // Loaded, nothing sent.
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(h.state().tabs()[0].sent.is_empty());
    type_into(&mut h, "Height", "3");
    settled(&mut h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(
        fields_of(&text_of(&f, &h), "Pin"),
        Some(expected(
            r#"{"type": "cylinder", "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 312}"#
        ))
    );
    assert_eq!(
        h.state().tabs()[0].sent,
        [Sent::Begin, Sent::Edit("height".to_owned())]
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn edit_feature_from_the_context_menu_opens_the_same_dialog() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    h.get_by_role_and_label(Role::Pane, "Timeline")
        .get_by_label("Block")
        .click_secondary();
    step(&mut h);
    step(&mut h);
    h.get_by_label("Edit Feature").click();
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Box").is_some()
    });
    for (label, value) in [("Length", "40"), ("Width", "20"), ("Height", "10")] {
        assert_eq!(input(&h, label).value().as_deref(), Some(value), "{label}");
    }
    // The cut has no dialog: its Edit Feature is off.
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    h.get_by_role_and_label(Role::Pane, "Timeline")
        .get_by_label("Hole")
        .click_secondary();
    step(&mut h);
    step(&mut h);
    assert!(
        h.get_by_label("Edit Feature")
            .accesskit_node()
            .is_disabled()
    );
    assert!(h.state().tabs()[0].sent.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_cutting_sweeps_are_off_with_no_body_to_cut() {
    let f = fixture();
    let only_sketch = SKETCHED
        .lines()
        .filter(|line| !line.contains(r#""type": "box""#))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut h, _) = f.opened("sketch.silico", &only_sketch);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    let create = h.get_by_role_and_label(Role::Toolbar, "CREATE");
    let off = |kind: &str| create.get_by_label(kind).accesskit_node().is_disabled();
    for kind in ["Pocket", "Groove"] {
        assert!(off(kind), "{kind}");
    }
    for kind in [
        "Box", "Cylinder", "Sphere", "Cone", "Torus", "Pad", "Revolve",
    ] {
        assert!(!off(kind), "{kind}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_refused_values_mark_goes_with_its_dialog() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    type_into(&mut h, "Radius", "-");
    settled(&mut h);
    assert!(!ok_enabled(&h));
    // Nor is Enter OK.
    h.key_press(eframe::egui::Key::Enter);
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(dialog_open(&h));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    // Nothing of its was taken, nor anything before it: rolled back, nothing undone.
    assert!(
        !h.state().tabs()[0]
            .sent
            .iter()
            .any(|sent| matches!(sent, Sent::Undo(_)))
    );
    assert!(!h.state().tabs()[0].transaction);
    // Opened again on the same feature: its radius is the document's, and OK on.
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("2.5"));
    assert!(ok_enabled(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancel_an_undo_and_an_add_sent_while_a_preview_rebuilds_land_without_waiting_for_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let hold = f.kernel.hold(is_box);
    let rebuilding = |h: &mut AppHarness| {
        step_until(h, "the preview rebuilding", |h| {
            h.state().tabs()[0].preview_rebuilding()
        });
    };
    // Cancel: its rollback lands with the rebuild held.
    create(&mut h, "Box");
    rebuilding(&mut h);
    dialog(&h)
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Rollback));
    // OK'd with its preview still rebuilding, then Ctrl+Z: its undo lands too.
    create(&mut h, "Box");
    rebuilding(&mut h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    step_until(&mut h, "undoable", |h| h.state().tabs()[0].undoable());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    assert!(matches!(
        h.state().tabs()[0].sent.last(),
        Some(Sent::Undo(Some(_)))
    ));
    // OK'd with its preview still rebuilding, then another dialog: its add lands.
    create(&mut h, "Box");
    rebuilding(&mut h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    create(&mut h, "Sphere");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    // And File > Revert with the sphere's preview rebuilding: its rollback lands too.
    rebuilding(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_label("Revert").click();
    step_until(&mut h, "reverted", |h| {
        !h.state().tabs()[0].transaction && !h.state().tabs()[0].unsettled()
    });
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Rollback));
    hold.release();
    assert_eq!(text_of(&f, &h), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_while_edit_feature_reads_the_feature_opens_no_dialog() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let (hold, check) = hold_queue(&f, &h, 0);
    double_click(&mut h, "Timeline", "Pin");
    assert!(h.state().tabs()[0].in_dialog());
    // Reading it, one dialog at a time: CREATE's tools off (drawn before the timeline
    // took the double-click, so from the next frame).
    step(&mut h);
    assert!(
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label("Box")
            .accesskit_node()
            .is_disabled()
    );
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    hold.release();
    check.wait();
    settled(&mut h);
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(!dialog_open(&h));
    assert!(h.state().tabs()[0].dialog.is_none());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_dialog_and_create_are_off_while_the_tab_closes() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    create(&mut h, "Box");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    settled(&mut h);
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    step(&mut h);
    assert!(h.state().tabs()[0].closing());
    assert!(!ok_enabled(&h));
    assert!(
        dialog(&h)
            .get_by_role_and_label(Role::Button, "Cancel")
            .accesskit_node()
            .is_disabled()
    );
    // Nor does Esc cancel it: its undo would come after the close.
    let sent = h.state().tabs()[0].sent.clone();
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(dialog_open(&h));
    assert_eq!(h.state().tabs()[0].sent, sent);
    assert!(input(&h, "Height").accesskit_node().is_disabled());
    hold.release();
    check.wait();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn edit_feature_of_a_revolve_about_a_sketch_line_keeps_that_axis() {
    let f = fixture();
    let revolve = r#"    {"id": "r0000001", "type": "revolve", "v": 1, "name": "Turned", "sketch": "s0000001", "axis": "nc000000", "extent": {"type": "symmetric", "angle": 90}}"#;
    let text = SKETCHED.replace("\n  ]\n}", &format!(",\n{revolve}\n  ]\n}}"));
    let (mut h, _) = f.opened("turned.silico", &text);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Turned"));
    double_click(&mut h, "Timeline", "Turned");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Revolve").is_some()
    });
    // The line is offered as itself, chosen; the form and angle as written; the sketch's
    // name in its box.
    let chosen = |h: &AppHarness, label: &str| {
        dialog(h)
            .get_by_role_and_label(Role::Button, label)
            .accesskit_node()
            .toggled()
            == Some(eframe::egui::accesskit::Toggled::True)
    };
    assert!(chosen(&h, "nc000000"));
    assert!(!chosen(&h, "Y axis"));
    assert!(chosen(&h, "Symmetric"));
    assert_eq!(input(&h, "Angle").value().as_deref(), Some("90"));
    assert!(dialog(&h).query_by_label("Triangle").is_some());
    type_into(&mut h, "Angle", "1");
    settled(&mut h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(
        fields_of(&text_of(&f, &h), "Turned"),
        Some(expected(
            r#"{"type": "revolve", "name": "Turned", "sketch": "s0000001", "axis": "nc000000", "extent": {"type": "symmetric", "angle": 190}}"#
        ))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn redo_is_off_while_a_dialog_is_open() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    settled(&mut h);
    step_until(&mut h, "undoable", |h| h.state().tabs()[0].undoable());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    step_until(&mut h, "redoable", |h| h.state().tabs()[0].redoable());
    double_click(&mut h, "Timeline", "Block");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Box").is_some()
    });
    assert!(!h.state().tabs()[0].redoable());
    assert!(
        h.get_by_role_and_label(Role::Button, "Redo")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn revert_closes_an_open_dialog_with_the_edits_it_drops() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    step_until(&mut h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    settled(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_label("Revert").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), EXAMPLE);
    assert!(h.state().tabs()[0].dialog.is_none());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancel_before_the_add_answers_waits_for_it_and_rolls_it_back() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let before = text_of(&f, &h);
    // The add waits in the session's queue: the Cancel waits behind it in the tab's.
    let (hold, check) = hold_queue(&f, &h, 0);
    create(&mut h, "Box");
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    hold.release();
    check.wait();
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(
        h.state().tabs()[0].sent,
        [Sent::Begin, Sent::Add, Sent::Rollback]
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancel_with_nothing_of_the_dialogs_after_the_file_reloaded_undoes_nothing() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    settled(&mut h);
    // Changed on disk by another client, no edit held: the session reloads it.
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Theirs""#);
    std::fs::write(&path, &theirs).unwrap();
    step_until(&mut h, "the reload", |h| shows(h, "Timeline", "Theirs"));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(h.state().tabs()[0].sent.is_empty());
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_name_typed_after_a_dialog_opened_is_its_cancels_to_undo_never_before_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    // "a" waits in the session's queue, "b" in the tab's; then the dialog opens on the
    // Pin, and "c" is typed: it comes after the dialog's opening, not in place of "b".
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "a");
    type_name(&mut h, "b");
    double_click(&mut h, "Timeline", "Pin");
    type_name(&mut h, "c");
    hold.release();
    check.wait();
    settled(&mut h);
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    let typed = text_of(&f, &h);
    let name = names_of(&typed)[1].clone();
    assert_eq!(name.len(), "cbaPin".len(), "{name}");
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    // Undone back to the two letters typed before the dialog.
    let kept = names_of(&text_of(&f, &h))[1].clone();
    assert_eq!(kept.len(), "baPin".len(), "{kept}");
    assert_eq!(never_waits::waits(), 0);
}

/// The features' names in `text`, in order.
fn names_of(text: &str) -> Vec<String> {
    text.split(r#""name": ""#)
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap().to_owned())
        .collect()
}

// ---- Feature dialogs: #325's review ----

/// The dialog's feature's add answered.
fn added(h: &mut AppHarness) {
    step_until(h, "its add answered", |h| {
        h.state().tabs()[0]
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
}

#[test]
fn esc_after_typing_coalesced_behind_a_held_preview_stays_stopped() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let hold = f.kernel.hold(is_box);
    create(&mut h, "Box");
    added(&mut h);
    // Two keystrokes, one a frame: the preview's read asked while an edit is in flight
    // runs at the revision that edit comes to.
    type_into(&mut h, "Length", "12");
    settled(&mut h);
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    h.key_press(eframe::egui::Key::Escape);
    let stopped = |h: &AppHarness| {
        h.state().tabs()[0].status.as_ref().is_some_and(|status| {
            status.rebuild.state == silico_core::command::RebuildState::Stopped
        })
    };
    step_until(&mut h, "the rebuild stopped", stopped);
    for _ in 0..30 {
        step(&mut h);
        assert!(stopped(&h), "the stop was undone");
        assert!(!h.state().tabs()[0].preview_rebuilding());
    }
    assert!(dialog_open(&h));
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_sweeps_body_is_the_documents_tip_even_where_the_model_has_none_there() {
    // The plate's cut by the washer fails: the model's bodies are the washer and the plate,
    // but the plate is the cut's, and the document's tip is the cut.
    let f = fixture();
    f.kernel.refuse(|op| {
        matches!(op, Op::Boolean { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "the cut came apart"))
    });
    let cut = r#"    {"id": "c0000001", "type": "cut", "v": 1, "name": "Notch", "base": "b0000001", "tool": "b0000002"}"#;
    let text = SKETCHED.replace("\n  ]\n}", &format!(",\n{cut}\n  ]\n}}"));
    let (mut h, _) = f.opened("notched.silico", &text);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    step_until(&mut h, "the cut's failure", |h| {
        showing(h, "Timeline", "error").is_some()
    });
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step(&mut h);
    // Every build held from before the pocket's add: its preview waits, so the tab's own
    // timeline never lists the pocket until the release.
    let hold = f.kernel.hold(|_| true);
    create(&mut h, "Pocket");
    added(&mut h);
    let pocket_id = h.state().tabs()[0]
        .dialog
        .as_ref()
        .unwrap()
        .feature
        .clone()
        .unwrap();
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    // OK'd with its preview held (unbuilt): the next sweep's tip is the pocket.
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert!(
        h.state().tabs()[0]
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.features.as_ref())
            .is_some_and(|features| features.iter().all(|entry| entry.id != pocket_id)),
        "the tab's timeline lists the pocket already: nothing was held"
    );
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step(&mut h);
    create(&mut h, "Pad");
    added(&mut h);
    settled(&mut h);
    hold.release();
    let text = text_of(&f, &h);
    let pocket = fields_of(&text, "Pocket").unwrap();
    assert!(
        pocket.contains(&("base".to_owned(), Plain::String("c0000001".to_owned()))),
        "{pocket:?}"
    );
    let pad = fields_of(&text, "Pad").unwrap();
    assert!(
        pad.contains(&(
            "base".to_owned(),
            Plain::String(pocket_id.as_str().to_owned())
        )),
        "{pad:?}"
    );
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn typing_while_the_add_is_unanswered_makes_one_feature_and_edits_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let (hold, check) = hold_queue(&f, &h, 0);
    create(&mut h, "Box");
    type_into(&mut h, "Length", "12");
    let typed = input(&h, "Length").value().unwrap();
    hold.release();
    check.wait();
    settled(&mut h);
    let text = text_of(&f, &h);
    assert_eq!(text.matches(r#""type": "box""#).count(), 2, "{text}");
    let fields = fields_of(&text, "Box").unwrap();
    assert!(
        fields.contains(&("length".to_owned(), Plain::Number(typed.clone()))),
        "{typed}: {fields:?}"
    );
    assert_eq!(
        h.state().tabs()[0]
            .sent
            .iter()
            .filter(|sent| **sent == Sent::Add)
            .count(),
        1
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_reload_under_edit_feature_reads_the_feature_again_and_cancel_undoes_from_there() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    settled(&mut h);
    let theirs = EXAMPLE.replace(r#""radius": 2.5"#, r#""radius": 3"#);
    std::fs::write(&path, &theirs).unwrap();
    step_until(&mut h, "the reloaded radius", |h| {
        dialog_open(h) && input(h, "Radius").value().as_deref() == Some("3")
    });
    type_into(&mut h, "Height", "1");
    settled(&mut h);
    assert!(text_of(&f, &h).contains(r#""height": 112"#));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), theirs);
    assert_eq!(h.state().tabs()[0].message, None);
    assert!(!h.state().tabs()[0].transaction);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_reload_under_a_new_sweeps_dialog_opens_it_at_the_reloaded_revision() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    let sketched = text_of(&f, &h);
    // A KPlate typed first, saved: a transaction with something before the dialog is
    // then the reload's to make, not the dialog's.
    select(&mut h, "Plate");
    type_name(&mut h, "K");
    settled(&mut h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "saved", |h| !h.state().tabs()[0].unsaved());
    assert_ne!(text_of(&f, &h), sketched);
    // The Pad's dialog, with nothing to add yet (no sketch chosen).
    create(&mut h, "Pad");
    let theirs = std::fs::read_to_string(&path)
        .unwrap()
        .replace(r#""name": "Washer""#, r#""name": "Theirs""#);
    std::fs::write(&path, &theirs).unwrap();
    step_until(&mut h, "the reload", |h| shows(h, "Browser", "Theirs"));
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    added(&mut h);
    settled(&mut h);
    // OK'd, then one undo: back to the reloaded file, the dialog's edits whole.
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    step_until(&mut h, "undoable", |h| h.state().tabs()[0].undoable());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), theirs);
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_suppressed_feature_is_never_a_sweeps_body() {
    let f = fixture();
    let ghost = r#"    {"id": "g0000001", "type": "box", "v": 1, "name": "Ghost", "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let text = SKETCHED.replace("\n  ]\n}", &format!(",\n{ghost}\n  ]\n}}"));
    let (mut h, _) = f.opened("ghost.silico", &text);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Ghost"));
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step(&mut h);
    create(&mut h, "Pad");
    added(&mut h);
    settled(&mut h);
    let pad = fields_of(&text_of(&f, &h), "Pad").unwrap();
    assert!(
        pad.contains(&("base".to_owned(), Plain::String("b0000001".to_owned()))),
        "{pad:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn two_dialogs_okd_are_undone_and_redone_each_whole() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let before = text_of(&f, &h);
    let mut texts = vec![before.clone()];
    for (kind, label) in [("Box", "Length"), ("Sphere", "Radius")] {
        create(&mut h, kind);
        added(&mut h);
        type_into(&mut h, label, "1");
        settled(&mut h);
        dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
        step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
        settled(&mut h);
        texts.push(text_of(&f, &h));
    }
    let key = |h: &mut AppHarness, undo: bool| {
        let (key, able): (_, fn(&AppHarness) -> bool) = if undo {
            (eframe::egui::Key::Z, |h| h.state().tabs()[0].undoable())
        } else {
            (eframe::egui::Key::Y, |h| h.state().tabs()[0].redoable())
        };
        step_until(h, "able", able);
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, key);
        step(h);
        settled(h);
    };
    // Z, Z: each dialog's edits whole.
    key(&mut h, true);
    assert_eq!(text_of(&f, &h), texts[1]);
    key(&mut h, true);
    assert_eq!(text_of(&f, &h), texts[0]);
    // Y, Y: each whole again.
    key(&mut h, false);
    assert_eq!(text_of(&f, &h), texts[1]);
    key(&mut h, false);
    assert_eq!(text_of(&f, &h), texts[2]);
    // And Z, Z once more.
    key(&mut h, true);
    assert_eq!(text_of(&f, &h), texts[1]);
    key(&mut h, true);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_is_off_while_a_dialog_is_open() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    assert!(h.state().tabs()[0].unsaved());
    assert!(
        h.get_by_role_and_label(Role::Button, "Save")
            .accesskit_node()
            .is_disabled()
    );
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    // OK'd: Save on.
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    step_until(&mut h, "Save on", |h| {
        !h.get_by_role_and_label(Role::Button, "Save")
            .accesskit_node()
            .is_disabled()
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn edit_features_read_comes_after_the_edits_asked_before_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    settled(&mut h);
    // The queue held: one radius in the session's queue, the next in the tab's, then OK and
    // Edit Feature again.
    let (hold, check) = hold_queue(&f, &h, 0);
    type_into(&mut h, "Radius", "12");
    let typed = input(&h, "Radius").value().unwrap();
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    double_click(&mut h, "Timeline", "Pin");
    hold.release();
    check.wait();
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    assert_eq!(input(&h, "Radius").value(), Some(typed));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_of_a_dialog_that_began_the_transaction_leaves_none_open_so_outside_writes_reload() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    assert!(h.state().tabs()[0].transaction);
    dialog(&h)
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert!(!h.state().tabs()[0].transaction);
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Theirs""#);
    std::fs::write(&path, &theirs).unwrap();
    step_until(&mut h, "the reload", |h| shows(h, "Timeline", "Theirs"));
    assert_eq!(text_of(&f, &h), theirs);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_add_answered_after_its_dialog_was_cancelled_never_becomes_the_next_dialogs() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    let before = text_of(&f, &h);
    let (hold, check) = hold_queue(&f, &h, 0);
    create(&mut h, "Box");
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    // A Pad's, with no sketch chosen: nothing of its own to add.
    create(&mut h, "Pad");
    hold.release();
    check.wait();
    settled(&mut h);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].dialog.as_ref().unwrap().feature, None);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_digit_typed_in_the_frame_enter_is_pressed_is_in_what_ok_keeps() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    input(&h, "Height").focus();
    step(&mut h);
    {
        use eframe::egui::{Event, Key, Modifiers};
        let events = &mut h.input_mut().events;
        events.push(Event::Text("7".to_owned()));
        events.push(Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
    step(&mut h);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    let fields = fields_of(&text_of(&f, &h), "Box").unwrap();
    assert!(
        fields.contains(&("height".to_owned(), Plain::Number("710".to_owned()))),
        "{fields:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- Feature dialogs: #325's second round ----

/// The Pad's base, made from `text` opened, its triangle sketch chosen.
fn pads_base(text: &str) -> Plain {
    let f = fixture();
    let (mut h, _) = f.opened("based.silico", text);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step(&mut h);
    create(&mut h, "Pad");
    added(&mut h);
    settled(&mut h);
    let pad = fields_of(&text_of(&f, &h), "Pad").unwrap();
    assert_eq!(never_waits::waits(), 0);
    pad.into_iter()
        .find(|(key, _)| key == "base")
        .map(|(_, base)| base)
        .unwrap_or(Plain::Null)
}

#[test]
fn a_suppressed_feature_whose_chain_stands_is_the_tip_and_a_consumed_one_never_is() {
    // A pocket of the plate, suppressed: nothing consumes it, and the plate stands for it,
    // so it's the tip (the gate takes it as a base, and it builds on the plate).
    let hollow = r#"    {"id": "h0000001", "type": "pocket", "v": 1, "name": "Hollow", "suppressed": true, "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 1, "reversed": false}}"#;
    let text = SKETCHED.replace("\n  ]\n}", &format!(",\n{hollow}\n  ]\n}}"));
    assert_eq!(pads_base(&text), Plain::String("h0000001".to_owned()));
    // The plate, then the washer, last of the bodies that stand, but a suppressed cut
    // (of a suppressed box, so it stands for nothing) consumes it: the tip is the plate.
    let consumed = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000001", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, -10], "length": 40, "width": 40, "height": 10},
    {"id": "b0000002", "type": "box", "v": 1, "name": "Washer", "position": [50, 0, 0], "length": 5, "width": 5, "height": 1},
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 20, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []},
    {"id": "g0000001", "type": "box", "v": 1, "name": "Ghost", "suppressed": true, "position": [0, 0, 0], "length": 1, "width": 1, "height": 1},
    {"id": "c0000002", "type": "cut", "v": 1, "name": "Snip", "suppressed": true, "base": "g0000001", "tool": "b0000002"}
  ]
}
"#;
    assert_eq!(pads_base(consumed), Plain::String("b0000001".to_owned()));
}

/// Ctrl+Z (`undo`) or Ctrl+Y, once it's on, and everything answered.
fn step_key(h: &mut AppHarness, undo: bool) {
    if undo {
        step_until(h, "undoable", |h| h.state().tabs()[0].undoable());
    } else {
        step_until(h, "redoable", |h| h.state().tabs()[0].redoable());
    }
    let key = if undo {
        eframe::egui::Key::Z
    } else {
        eframe::egui::Key::Y
    };
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, key);
    step(h);
    settled(h);
}

/// A `kind` dialog: added, `label` typed into, and OK'd.
fn made(h: &mut AppHarness, kind: &str, label: &str) {
    create(h, kind);
    added(h);
    type_into(h, label, "1");
    settled(h);
    dialog(h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(h, "the dialog closed", |h| !dialog_open(h));
    settled(h);
}

#[test]
fn a_dialog_undone_then_another_made_lets_the_other_be_redone_whole() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let before = text_of(&f, &h);
    made(&mut h, "Box", "Length");
    step_key(&mut h, true);
    assert_eq!(text_of(&f, &h), before);
    // The box's edits are the redo branch the sphere's add drops.
    made(&mut h, "Sphere", "Radius");
    let sphere = text_of(&f, &h);
    step_key(&mut h, true);
    assert_eq!(text_of(&f, &h), before);
    step_key(&mut h, false);
    assert_eq!(text_of(&f, &h), sphere);
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_dialog_undone_then_a_name_typed_lets_the_name_be_redone() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let before = text_of(&f, &h);
    made(&mut h, "Box", "Length");
    step_key(&mut h, true);
    assert_eq!(text_of(&f, &h), before);
    // A name typed in the property panel drops the box's edits.
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    settled(&mut h);
    let named = text_of(&f, &h);
    assert!(named.contains(r#""name": "KPin""#));
    step_key(&mut h, true);
    assert_eq!(text_of(&f, &h), before);
    step_key(&mut h, false);
    assert_eq!(text_of(&f, &h), named);
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Redo(None)));
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ok_and_enter_wait_for_the_add_so_what_was_typed_meanwhile_is_kept() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let (hold, check) = hold_queue(&f, &h, 0);
    create(&mut h, "Box");
    type_into(&mut h, "Length", "12");
    let typed = input(&h, "Length").value().unwrap();
    // The add unanswered: OK off, and Enter does nothing.
    assert!(!ok_enabled(&h));
    h.key_press(eframe::egui::Key::Enter);
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(dialog_open(&h));
    hold.release();
    check.wait();
    added(&mut h);
    settled(&mut h);
    step_until(&mut h, "OK on", ok_enabled);
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    let fields = fields_of(&text_of(&f, &h), "Box").unwrap();
    assert!(
        fields.contains(&("length".to_owned(), Plain::Number(typed.clone()))),
        "{typed}: {fields:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn edit_feature_opens_at_the_revision_its_read_found_when_the_session_moved_under_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let id = h.state().tabs()[0].id;
    // The queue held: another client's edit of the Pin (outside a transaction, saved as
    // its own), then Edit Feature's read, behind it.
    let (hold, check) = hold_queue(&f, &h, 0);
    let pin = silico_core::document::FeatureId::new("a91cw0de").unwrap();
    let theirs = f.sessions.set_field(
        &id,
        h.state().tabs()[0].revision,
        pin,
        "height".to_owned(),
        json::read(b"15").unwrap(),
    );
    double_click(&mut h, "Timeline", "Pin");
    hold.release();
    check.wait();
    theirs.wait().result.unwrap();
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    assert_eq!(input(&h, "Height").value().as_deref(), Some("15"));
    settled(&mut h);
    let moved = text_of(&f, &h);
    // A change, OK, then one undo: back to what the read found, not refused.
    type_into(&mut h, "Radius", "1");
    settled(&mut h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    step_key(&mut h, true);
    assert_eq!(text_of(&f, &h), moved);
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_is_off_while_a_dialog_is_open() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    // File > Save As… off, and Ctrl+Shift+S asks nothing.
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    assert!(
        h.get_by_role_and_label(Role::Button, "Save As…")
            .accesskit_node()
            .is_disabled()
    );
    // The menu closed by its own button (Esc is the dialog's Cancel).
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.key_press_modifiers(
        eframe::egui::Modifiers::COMMAND | eframe::egui::Modifiers::SHIFT,
        eframe::egui::Key::S,
    );
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(!anywhere(&h, "Save example.silico as"));
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert!(dialog_open(&h));
    // OK'd: Save As offered again.
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    h.key_press_modifiers(
        eframe::egui::Modifiers::COMMAND | eframe::egui::Modifiers::SHIFT,
        eframe::egui::Key::S,
    );
    step_until(&mut h, "the Save As dialog", |h| {
        anywhere(h, "Save example.silico as")
    });
    assert_eq!(never_waits::waits(), 0);
}

// ---- Feature dialogs: #325's third round ----

#[test]
fn a_refused_edit_after_an_undone_dialog_leaves_its_redo_whole() {
    let f = fixture();
    let one_letter = EXAMPLE.replace(r#""name": "Pin""#, r#""name": "P""#);
    let (mut h, _) = f.opened("example.silico", &one_letter);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "P"));
    made(&mut h, "Box", "Length");
    let boxed = text_of(&f, &h);
    step_key(&mut h, true);
    assert_eq!(text_of(&f, &h), one_letter);
    // The one-letter name emptied: refused, so the box's edits stay the redo branch.
    select(&mut h, "P");
    name_field(&h).focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    step_until(&mut h, "the empty name refused", |h| {
        h.state().tabs()[0].message.as_deref() == Some("name is empty (give the feature a name)")
            && !h.state().tabs()[0].unsettled()
    });
    settled(&mut h);
    step_key(&mut h, false);
    assert_eq!(text_of(&f, &h), boxed);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_sweeps_body_read_before_the_session_moved_is_read_again() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    // The Pad's dialog, no sketch chosen: nothing to add yet.
    create(&mut h, "Pad");
    settled(&mut h);
    let id = h.state().tabs()[0].id;
    // Another client's box, the document's tip once it's added, held in the session's
    // queue.
    let (hold, check) = hold_queue(&f, &h, 0);
    let theirs = f.sessions.add_feature(
        &id,
        h.state().tabs()[0].revision,
        None,
        json::read(
            br#"{"type": "box", "name": "Theirs", "position": [90, 0, 0], "length": 2, "width": 2, "height": 2}"#,
        )
        .unwrap(),
    );
    // The sketch chosen with no frame drawn (a frame would take the read's answer): the
    // Pad's body is read, beside the queue, at the revision before the box.
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        tab.pick(&silico_core::document::FeatureId::new("s0000001").unwrap());
        tab.dialog_changed(&sessions);
        assert!(tab.reading_body());
    }
    // Answered (no published read is left running), not taken: the session then moves.
    let deadline = Instant::now() + WAIT;
    while !f.sessions.status(&id).result.unwrap().reads.is_empty() {
        assert!(Instant::now() < deadline, "the body's read never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state().tabs()[0].reading_body());
    hold.release();
    check.wait();
    let theirs = theirs.wait().result.unwrap().feature.id;
    added(&mut h);
    settled(&mut h);
    let pad = fields_of(&text_of(&f, &h), "Pad").unwrap();
    assert!(
        pad.contains(&("base".to_owned(), Plain::String(theirs.as_str().to_owned()))),
        "{pad:?}"
    );
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_that_changes_nothing_after_an_undone_dialog_leaves_its_redo_whole() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    made(&mut h, "Box", "Length");
    let boxed = text_of(&f, &h);
    step_key(&mut h, true);
    assert_eq!(text_of(&f, &h), EXAMPLE);
    // The Pin's radius made "-2.5" (refused), then "2.5" again: a set_field of the value
    // the document holds, which changes nothing.
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    type_into(&mut h, "Radius", "-");
    settled(&mut h);
    let sent = h.state().tabs()[0].edits.sent;
    h.key_press(eframe::egui::Key::Home);
    step(&mut h);
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("2.5"));
    assert_eq!(h.state().tabs()[0].edits.sent, sent + 1);
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), EXAMPLE);
    step_key(&mut h, false);
    assert_eq!(text_of(&f, &h), boxed);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_refused_because_another_client_undid_first_keeps_an_undone_dialogs_redo() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    settled(&mut h);
    made(&mut h, "Box", "Length");
    let boxed = text_of(&f, &h);
    step_key(&mut h, true);
    select(&mut h, "KPin");
    // The queue held: another client's undo (of KPin), then a name typed here against the
    // revision before it, refused as stale, the session's revision moved by the undo,
    // which drops no redo branch.
    let id = h.state().tabs()[0].id;
    let (hold, check) = hold_queue(&f, &h, 0);
    let theirs = f.sessions.undo(&id, h.state().tabs()[0].revision, None);
    type_name(&mut h, "Q");
    hold.release();
    check.wait();
    theirs.wait().result.unwrap();
    step_until(&mut h, "the stale name refused", |h| {
        h.state().tabs()[0].message.is_some() && !h.state().tabs()[0].unsettled()
    });
    settled(&mut h);
    assert_eq!(text_of(&f, &h), EXAMPLE);
    // Redo: KPin, then the box's edits whole.
    step_key(&mut h, false);
    step_key(&mut h, false);
    assert_eq!(text_of(&f, &h), boxed);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_refused_body_read_is_read_again_and_no_add_goes_until_it_answers() {
    use silico_core::command::{MAX_PUBLISHED_READS, ReadCommand};
    use std::sync::mpsc;
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    // The Pad's dialog, no sketch chosen: nothing to add yet.
    create(&mut h, "Pad");
    settled(&mut h);
    let id = h.state().tabs()[0].id;
    // As many published reads as a session runs at once, each held until released: a body's
    // read past them is refused (too many reads).
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(Mutex::new(released));
    let (started, starts) = mpsc::channel::<()>();
    let held: Vec<_> = (0..MAX_PUBLISHED_READS)
        .map(|_| {
            let released = Arc::clone(&released);
            let started = started.clone();
            f.sessions
                .read_model(&id, ReadCommand::Check, ModelChoice::Published, move |_| {
                    started.send(()).unwrap();
                    let _ = released.lock().unwrap().recv();
                    Ok(())
                })
        })
        .collect();
    for _ in 0..MAX_PUBLISHED_READS {
        starts.recv_timeout(WAIT).unwrap();
    }
    // The sketch chosen: the body's read is sent, and refused.
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        tab.pick(&silico_core::document::FeatureId::new("s0000001").unwrap());
        tab.dialog_changed(&sessions);
        assert!(tab.reading_body());
    }
    // Refused each time it's read: no add sent, the dialog still adding.
    for _ in 0..20 {
        step(&mut h);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!h.state().tabs()[0].sent.contains(&Sent::Add));
    assert!(h.state().tabs()[0].dialog.as_ref().unwrap().adding);
    assert!(h.state().tabs()[0].reading_body());
    // Released: read, and added on the document's tip, the plate.
    drop(release);
    for read in held {
        read.wait().result.unwrap();
    }
    added(&mut h);
    settled(&mut h);
    let pad = fields_of(&text_of(&f, &h), "Pad").unwrap();
    assert!(
        pad.contains(&("base".to_owned(), Plain::String("b0000001".to_owned()))),
        "{pad:?}"
    );
    assert_eq!(
        h.state().tabs()[0]
            .sent
            .iter()
            .filter(|s| **s == Sent::Add)
            .count(),
        1
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- The stop: `cancel` naming the rebuild (decision 34, as amended) ----

/// The Block made 41 long, through the tab, with every box held: its rebuild runs, started
/// by the frame's published read, no command waiting on it.
fn edit_the_block_held(f: &Fixture, h: &mut AppHarness) -> Hold {
    let hold = f.kernel.hold(is_box);
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        tab.edit(
            &sessions,
            silico_app::tab::Edit {
                feature: silico_core::document::FeatureId::new("k7f2qx3m").unwrap(),
                field: "length".to_owned(),
                value: json::read(b"41").unwrap(),
            },
        );
    }
    step_until(h, "the rebuild running, no command waiting", |h| {
        h.state().tabs()[0].transaction
            && h.state().tabs()[0].status.as_ref().is_some_and(|status| {
                status.rebuild.state == silico_core::command::RebuildState::Running
                    && status.running.is_none()
            })
    });
    hold
}

fn rebuild_stopped(h: &AppHarness) -> bool {
    h.state().tabs()[0]
        .status
        .as_ref()
        .is_some_and(|status| status.rebuild.state == silico_core::command::RebuildState::Stopped)
}

#[test]
fn stop_is_on_while_a_rebuild_runs_with_no_command_and_stops_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let hold = edit_the_block_held(&f, &mut h);
    let stop = |h: &AppHarness| {
        h.query_all_by_role_and_label(Role::Button, "Stop")
            .last()
            .unwrap()
            .accesskit_node()
            .is_disabled()
    };
    step_until(&mut h, "Stop on", |h| !stop(h));
    // A refusal said before: a stop that succeeds clears it.
    h.state_mut().tabs_mut()[0].message = Some("an earlier refusal".to_owned());
    h.query_all_by_role_and_label(Role::Button, "Stop")
        .last()
        .unwrap()
        .click();
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    // Frames climb, the stop holds, and nothing was refused.
    let before = h.state().frames();
    for _ in 0..30 {
        step(&mut h);
        assert!(rebuild_stopped(&h));
    }
    assert_eq!(h.state().frames(), before + 30);
    assert!(stop(&h));
    assert_eq!(h.state().tabs()[0].message, None);
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_stops_a_rebuild_running_with_no_command_and_with_none_does_nothing() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    // Nothing running: Esc says nothing.
    h.key_press(eframe::egui::Key::Escape);
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].message, None);
    let hold = edit_the_block_held(&f, &mut h);
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(rebuild_stopped(&h));
    assert_eq!(h.state().tabs()[0].message, None);
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

// ---- #325's mutants: the frame asked for, a toggle loaded, a pick at once ----

/// Whether the last frame asked for the next at once (`request_repaint`), not the idle one.
fn asked_at_once(h: &AppHarness) -> bool {
    h.output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .is_some_and(|viewport| viewport.repaint_delay == Duration::ZERO)
}

/// Frames stepped until one asks for none at once (nothing pending answers into it).
fn idle(h: &mut AppHarness) {
    step_until(h, "an idle frame", |h| !asked_at_once(h));
}

#[test]
fn an_open_dialog_with_nothing_to_send_asks_for_no_frame_at_once() {
    // A frame with input in it is asked for again by egui itself; one with none, while a
    // dialog sits open with nothing changed, asks for no frame at once (only the idle one).
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    quiet_wakes(&mut h);
    idle(&mut h);
    // A frame the core's waker asked for (an answer coming in late) may ask for the next;
    // one it didn't, mustn't.
    for _ in 0..10 {
        let woken = h.state().wakes();
        step(&mut h);
        assert!(!asked_at_once(&h) || h.state().wakes() != woken);
    }
    assert!(dialog_open(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn edit_feature_of_a_reversed_pad_loads_it_reversed_and_sends_nothing() {
    let f = fixture();
    let raised = r#"    {"id": "r0000002", "type": "pad", "v": 1, "name": "Raised", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 3, "reversed": true}}"#;
    let text = SKETCHED.replace("\n  ]\n}", &format!(",\n{raised}\n  ]\n}}"));
    let (mut h, _) = f.opened("raised.silico", &text);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Raised"));
    double_click(&mut h, "Timeline", "Raised");
    step_until(&mut h, "its dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Pad").is_some()
    });
    assert_eq!(
        dialog(&h)
            .get_by_role(Role::CheckBox)
            .accesskit_node()
            .toggled(),
        Some(eframe::egui::accesskit::Toggled::True)
    );
    assert_eq!(input(&h, "Length").value().as_deref(), Some("3"));
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(h.state().tabs()[0].sent.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_sketch_chosen_in_the_browser_fills_the_box_in_the_frame_it_was_chosen() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    create(&mut h, "Pad");
    settled(&mut h);
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step(&mut h);
    let dialog = h.state().tabs()[0].dialog.clone().unwrap();
    let sketch = silico_core::document::FeatureId::new("s0000001").unwrap();
    assert_eq!(
        dialog.inputs[dialog.active.unwrap()].value,
        silico_app::dialog::Value::Selection(Some(sketch))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tab_is_unsettled_while_anything_of_its_is_under_way() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    settled(&mut h);
    assert!(!h.state().tabs()[0].unsettled());
    select(&mut h, "Pin");
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "K");
    for _ in 0..5 {
        step(&mut h);
        assert!(h.state().tabs()[0].unsettled());
    }
    hold.release();
    check.wait();
    settled(&mut h);
    assert!(!h.state().tabs()[0].unsettled());
    assert_eq!(never_waits::waits(), 0);
}

/// Every request held from the open: the first rebuild runs, and Stop clicked stops it,
/// nothing published.
fn first_rebuild_stopped(f: &Fixture) -> (AppHarness, Hold) {
    first_rebuild_stopped_in(f, f.app())
}

/// [`first_rebuild_stopped`], in `h`.
fn first_rebuild_stopped_in(f: &Fixture, mut h: AppHarness) -> (AppHarness, Hold) {
    let hold = f.kernel.hold(|_| true);
    let path = f.file("example.silico", EXAMPLE);
    h.state_mut().open(path.to_str().unwrap(), false);
    let stop = |h: &AppHarness| {
        h.query_all_by_role_and_label(Role::Button, "Stop")
            .last()
            .is_some_and(|stop| !stop.accesskit_node().is_disabled())
    };
    step_until(&mut h, "Stop on", stop);
    h.query_all_by_role_and_label(Role::Button, "Stop")
        .last()
        .unwrap()
        .click();
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    (h, hold)
}

/// Stepped until the core has nothing more to tell the app: its wakes flat over ten frames,
/// each given a little while for an answer to come.
fn quiet_wakes(h: &mut AppHarness) {
    let deadline = Instant::now() + WAIT;
    loop {
        let before = h.state().wakes();
        for _ in 0..10 {
            step(h);
            std::thread::sleep(Duration::from_millis(5));
        }
        if h.state().wakes() == before {
            return;
        }
        assert!(Instant::now() < deadline, "the wakes never settled");
    }
}

/// How many times the fake was asked for a box.
fn boxes_asked(f: &Fixture) -> usize {
    f.kernel
        .calls()
        .iter()
        .filter(|call| is_box(&call.op))
        .count()
}

#[test]
fn a_stop_of_the_first_rebuild_holds_with_nothing_published() {
    // The first rebuild stopped, nothing published: its timeline isn't read of the current
    // model again (which would start the rebuild again), so the stop holds until an edit,
    // or Rebuild.
    let f = fixture();
    let (mut h, hold) = first_rebuild_stopped(&f);
    quiet_wakes(&mut h);
    assert!(rebuild_stopped(&h));
    assert_eq!(boxes_asked(&f), 1);
    assert_eq!(h.state().tabs()[0].message, None);
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_stopped_rebuild_with_nothing_published_reads_nothing_more_and_asks_for_no_frame() {
    // A read of the published model is refused for want of one, and one of the current model
    // would undo the stop: nothing is read, nothing wakes the app, and the frame asked for is
    // the idle one.
    let f = fixture();
    // Time barely moves from frame to frame, so the frame asked for reads as it was asked.
    let h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .with_step_dt(0.001)
            .build_ui_state(
                |ui, app: &mut App<Fake>| app.frame(ui),
                App::new(Arc::clone(&f.sessions)),
            )
    };
    let (mut h, hold) = first_rebuild_stopped_in(&f, h);
    quiet_wakes(&mut h);
    let woken = h.state().wakes();
    for _ in 0..30 {
        step(&mut h);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(h.state().wakes(), woken);
    let delay = h
        .output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .map(|viewport| viewport.repaint_delay);
    assert!(
        delay.is_some_and(|delay| {
            delay > silico_core::disk::DISK_CHECK_INTERVAL && delay <= IDLE_FRAME
        }),
        "{delay:?}"
    );
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

/// From the first rebuild stopped: the panes say so, `rebuild` (Rebuild clicked, or Ctrl+B)
/// starts it again, from a read of the current model, and once it's built the timeline
/// shows and Rebuild is off.
fn rebuilt_after_the_stop(rebuild: impl Fn(&mut AppHarness)) {
    let f = fixture();
    let (mut h, hold) = first_rebuild_stopped(&f);
    quiet_wakes(&mut h);
    let rebuild_on = |h: &AppHarness| {
        !h.get_by_role_and_label(Role::Button, "Rebuild")
            .accesskit_node()
            .is_disabled()
    };
    assert!(rebuild_on(&h));
    assert!(shows(&h, "Timeline", "Rebuild stopped"));
    assert!(shows(&h, "Browser", "Rebuild stopped"));
    assert_eq!(boxes_asked(&f), 1);
    rebuild(&mut h);
    step_until(&mut h, "the box asked again", |_| boxes_asked(&f) == 2);
    hold.release();
    step_until(&mut h, "the timeline", |h| shows(h, "Timeline", "Hole"));
    assert!(!rebuild_on(&h));
    assert!(!rebuild_stopped(&h));
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn rebuild_clicked_after_a_stop_rebuilds_and_shows_the_timeline() {
    rebuilt_after_the_stop(|h| {
        h.get_by_role_and_label(Role::Button, "Rebuild").click();
        step(h);
    });
}

#[test]
fn ctrl_b_after_a_stop_rebuilds_and_shows_the_timeline() {
    rebuilt_after_the_stop(|h| {
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::B);
        step(h);
    });
}

#[test]
fn esc_clears_the_selection_before_it_stops_the_rebuild() {
    // Section 3's order: with a selection held and a rebuild held, Esc clears the selection;
    // a second Esc stops the rebuild.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    let hold = edit_the_block_held(&f, &mut h);
    h.state_mut().tabs_mut()[0].selected =
        Some(silico_core::document::FeatureId::new("a91cw0de").unwrap());
    step(&mut h);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert_eq!(h.state().tabs()[0].selected, None);
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(!rebuild_stopped(&h));
    assert!(h.state().tabs()[0].stoppable());
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_b_while_a_rebuild_runs_reads_nothing() {
    // Rebuild is only for a stopped rebuild: with one running, Ctrl+B asks nothing (a read
    // of the current model would be the command running, waiting on it).
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let hold = edit_the_block_held(&f, &mut h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::B);
    for _ in 0..10 {
        step(&mut h);
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(
            h.state().tabs()[0]
                .status
                .as_ref()
                .and_then(|status| status.running),
            None
        );
    }
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn rebuild_twice_before_the_session_starts_it_sends_one_read() {
    let f = fixture();
    let (mut h, hold) = first_rebuild_stopped(&f);
    quiet_wakes(&mut h);
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        assert!(tab.can_rebuild());
        tab.rebuild(&sessions);
        // Its read out: Rebuild off, and a second sends nothing.
        assert!(!tab.can_rebuild());
        tab.rebuild(&sessions);
        assert_eq!(tab.rebuilds, 1);
    }
    step(&mut h);
    assert!(
        h.get_by_role_and_label(Role::Button, "Rebuild")
            .accesskit_node()
            .is_disabled()
    );
    hold.release();
    step_until(&mut h, "the timeline", |h| shows(h, "Timeline", "Hole"));
    assert_eq!(h.state().tabs()[0].rebuilds, 1);
    // Stopped again (an edit's rebuild): Rebuild is on again.
    let hold = edit_the_block_held(&f, &mut h);
    h.query_all_by_role_and_label(Role::Button, "Stop")
        .last()
        .unwrap()
        .click();
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    step_until(&mut h, "Rebuild on", |h| {
        !h.get_by_role_and_label(Role::Button, "Rebuild")
            .accesskit_node()
            .is_disabled()
    });
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

// ---- The export dialog (M5.12; M4.1's `export`) ----

/// File > Export…, its dialog open on the tab's document.
fn open_export(h: &mut AppHarness) {
    h.get_by_label("File").click();
    step(h);
    step(h);
    h.get_by_role_and_label(Role::Button, "Export…").click();
    step_until(h, "the export dialog", |h| {
        anywhere(h, "Export example.silico")
    });
}

/// The export dialog's field `label` emptied (its cursor at its start once focused, a
/// Delete for each character), then `text` typed at once.
fn set_field(h: &mut AppHarness, label: &str, text: &str) {
    h.get_by_role_and_label(Role::TextInput, label).focus();
    step(h);
    let length = h
        .get_by_role_and_label(Role::TextInput, label)
        .value()
        .unwrap()
        .chars()
        .count();
    for _ in 0..length {
        h.key_press(eframe::egui::Key::Delete);
        step(h);
    }
    h.get_by_role_and_label(Role::TextInput, label)
        .type_text(text);
    step(h);
    assert_eq!(
        h.get_by_role_and_label(Role::TextInput, label)
            .value()
            .as_deref(),
        Some(text)
    );
}

/// The export dialog's button `label` clicked, through the accessibility tree (a modal's
/// layout moves as its content changes, so a click at its last place can miss).
fn press(h: &mut AppHarness, label: &str) {
    h.get_by_role_and_label(Role::Button, label)
        .click_accesskit();
    step(h);
    // Drawn again as what it did leaves it (a line said, a row gone): the next press finds
    // its button where it is, its ID among them.
    step(h);
}

/// Whether the export dialog is open.
fn exporting_dialog(h: &AppHarness) -> bool {
    anywhere(h, "Export example.silico")
}

fn is_step(op: &Op) -> bool {
    matches!(op, Op::ExportStep(_))
}

#[test]
fn an_export_held_leaves_frames_climbing_and_the_dialog_saying_so() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let hold = f.kernel.hold(is_step);
    open_export(&mut h);
    press(&mut h, "STEP");
    press(&mut h, "Export");
    assert!(f.kernel.wait_running(1, WAIT));
    let before = h.state().frames();
    for _ in 0..100 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), before + 100);
    assert!(anywhere(&h, "Exporting…"));
    assert!(h.state().tabs()[0].exporting());
    // Not the status bar's to stop: no rebuild runs.
    assert!(
        h.get_by_role_and_label(Role::Button, "Stop")
            .accesskit_node()
            .is_disabled()
    );
    // One at a time: Export off, and the tab sends no second.
    assert!(
        h.get_by_role_and_label(Role::Button, "Export")
            .accesskit_node()
            .is_disabled()
    );
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        let again = tab.exports[0].clone();
        tab.export(&sessions, again);
        assert_eq!(tab.exports.len(), 1);
    }
    hold.release();
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(path.with_extension("step").is_file());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn each_format_sends_its_request_its_tolerance_and_its_features() {
    use silico_core::command::export::{Export, ExportFormat};
    use silico_core::document::{FeatureId, Length};
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // A 3MF at 0.25 mm, of every body. The fake can't mesh: what it answers is its own
    // refusal, said in the dialog, which stays open, and no file is written. A 3MF written
    // is M5.12's scripted slice's, on a real kernel worker.
    open_export(&mut h);
    set_field(&mut h, "Tolerance", "0.25");
    press(&mut h, "Export");
    step_until(&mut h, "its answer", |h| {
        h.state().tabs()[0].export_answer.is_some()
    });
    assert_eq!(
        h.state().tabs()[0].exports,
        [Export {
            format: ExportFormat::ThreeMf {
                tolerance: Length::from_nm(250_000)
            },
            path: path.with_extension("3mf"),
            replace: false,
            features: None,
        }]
    );
    let refused = h.state().tabs()[0]
        .export_answer
        .clone()
        .unwrap()
        .unwrap_err();
    assert!(
        refused.errors()[0]
            .message()
            .contains("the fake kernel doesn't do ExportMesh"),
        "{refused:?}"
    );
    step(&mut h);
    assert!(exporting_dialog(&h));
    assert!(!path.with_extension("3mf").exists());
    // A STEP of the Hole alone (the model's one body), chosen.
    press(&mut h, "Cancel");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    open_export(&mut h);
    press(&mut h, "STEP");
    assert!(
        h.query_by_role_and_label(Role::TextInput, "Tolerance")
            .is_none()
    );
    h.get_by_role_and_label(Role::CheckBox, "All bodies")
        .click_accesskit();
    step_until(&mut h, "the bodies listed", |h| {
        h.query_by_role_and_label(Role::CheckBox, "Hole").is_some()
    });
    h.get_by_role_and_label(Role::CheckBox, "Hole")
        .click_accesskit();
    step(&mut h);
    press(&mut h, "Export");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert_eq!(
        h.state().tabs()[0].exports.last(),
        Some(&Export {
            format: ExportFormat::Step,
            path: path.with_extension("step"),
            replace: false,
            features: Some(vec![FeatureId::new("m3tq8v0z").unwrap()]),
        })
    );
    assert!(path.with_extension("step").is_file());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tolerance_out_of_range_is_refused_by_the_core_and_said_in_the_dialog() {
    use silico_core::command::export::{Export, ExportFormat};
    use silico_core::document::Length;
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // What the core says of a 3MF at 0 mm, below its least (1e-5 mm), asked directly.
    let id = h.state().tabs()[0].id;
    let core = f
        .sessions
        .export(
            &id,
            Export {
                format: ExportFormat::ThreeMf {
                    tolerance: Length::from_nm(0),
                },
                path: path.with_extension("3mf"),
                replace: false,
                features: None,
            },
        )
        .wait()
        .result
        .unwrap_err();
    let error = &core.errors()[0];
    assert_eq!(error.parameter(), Some("tolerance"));
    let want = match error.suggestion() {
        Some(suggestion) => format!("{} ({suggestion})", error.message()),
        None => error.message().to_owned(),
    };
    open_export(&mut h);
    set_field(&mut h, "Tolerance", "0");
    press(&mut h, "Export");
    // Said in the dialog, and in the status bar, as every command's refusal is.
    step_until(&mut h, "the refusal said", |h| {
        h.query_all_by_label(&want).count() == 2
    });
    assert!(exporting_dialog(&h));
    assert!(!path.with_extension("3mf").exists());
    // A tolerance that doesn't read as a length: Export off, nothing sent.
    set_field(&mut h, "Tolerance", "fine");
    assert!(
        h.get_by_role_and_label(Role::Button, "Export")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(h.state().tabs()[0].exports.len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_file_at_the_path_is_asked_about_then_replaced() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let out = path.with_extension("step");
    std::fs::write(&out, "old").unwrap();
    open_export(&mut h);
    press(&mut h, "STEP");
    press(&mut h, "Export");
    let asked = format!("{} already exists. Replace it?", out.display());
    step_until(&mut h, "the question", |h| anywhere(h, &asked));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "old");
    assert_eq!(h.state().tabs()[0].message, None);
    // Cancelled at the question: nothing written, and asked nothing when opened again.
    press(&mut h, "Cancel");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "old");
    open_export(&mut h);
    assert!(!anywhere(&h, &asked));
    press(&mut h, "STEP");
    press(&mut h, "Export");
    step_until(&mut h, "the question", |h| anywhere(h, &asked));
    press(&mut h, "Replace");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    // The fake's STEP, as the STEP worker would write it.
    assert!(
        std::fs::read_to_string(&out)
            .unwrap()
            .starts_with("FAKE STEP\n")
    );
    let exports = &h.state().tabs()[0].exports;
    assert_eq!(exports.len(), 3);
    assert!(!exports[0].replace && !exports[1].replace);
    assert!(exports[2].replace);
    assert_eq!(exports[2].path, out);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_while_an_export_runs_answers_cancelled_and_writes_no_file() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let hold = f.kernel.hold(is_step);
    open_export(&mut h);
    press(&mut h, "STEP");
    press(&mut h, "Export");
    assert!(f.kernel.wait_running(1, WAIT));
    step_until(&mut h, "running", |h| anywhere(h, "Exporting…"));
    press(&mut h, "Cancel");
    step_until(&mut h, "its answer", |h| {
        h.state().tabs()[0].export_answer.is_some()
    });
    assert!(!exporting_dialog(&h));
    let answer = h.state().tabs()[0].export_answer.clone().unwrap();
    assert_eq!(answer.unwrap_err().errors()[0].code(), ErrorCode::Cancelled);
    assert!(!path.with_extension("step").exists());
    assert_eq!(h.state().tabs()[0].message, None);
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_step_export_to_a_stp_path_is_written_there() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    open_export(&mut h);
    press(&mut h, "STEP");
    let stp = path.with_extension("stp");
    set_field(&mut h, "Path", stp.to_str().unwrap());
    press(&mut h, "Export");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(stp.is_file());
    assert!(!path.with_extension("step").exists());
    assert_eq!(h.state().tabs()[0].exports[0].path, stp);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_export_over_the_document_itself_is_refused_by_the_core_and_the_document_kept() {
    use silico_core::command::export::{Export, ExportFormat};
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // What the core says of a STEP written over the open document, asked directly.
    let id = h.state().tabs()[0].id;
    let core = f
        .sessions
        .export(
            &id,
            Export {
                format: ExportFormat::Step,
                path: path.clone(),
                replace: true,
                features: None,
            },
        )
        .wait()
        .result
        .unwrap_err();
    let error = &core.errors()[0];
    assert_eq!(error.parameter(), Some("path"));
    let want = match error.suggestion() {
        Some(suggestion) => format!("{} ({suggestion})", error.message()),
        None => error.message().to_owned(),
    };
    open_export(&mut h);
    press(&mut h, "STEP");
    set_field(&mut h, "Path", path.to_str().unwrap());
    press(&mut h, "Export");
    step_until(&mut h, "the refusal said", |h| {
        h.query_all_by_label(&want).count() == 2
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

// ---- The export dialog: #330's review ----

#[test]
fn cancel_of_an_export_queued_behind_a_commit_stops_it_and_writes_no_file() {
    // The export waits in the session's queue behind a commit, itself waiting on a held
    // rebuild: its Cancel stops it there (through its own handle, not by name, which only
    // reaches a command running), and nothing is written once the commit goes through.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let hold = edit_the_block_applied_held(&f, &mut h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    open_export(&mut h);
    press(&mut h, "STEP");
    press(&mut h, "Export");
    assert!(h.state().tabs()[0].exporting());
    press(&mut h, "Cancel");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    hold.release();
    step_until(&mut h, "its answer", |h| {
        h.state().tabs()[0].export_answer.is_some()
    });
    let answer = h.state().tabs()[0].export_answer.clone().unwrap();
    assert_eq!(answer.unwrap_err().errors()[0].code(), ErrorCode::Cancelled);
    step_until(&mut h, "saved", |h| !h.state().tabs()[0].unsaved());
    assert!(!path.with_extension("step").exists());
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

/// The Block made 41 long, through the tab, with every box held: its rebuild runs.
fn edit_the_block_applied_held(f: &Fixture, h: &mut AppHarness) -> Hold {
    let hold = f.kernel.hold(is_box);
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        tab.edit(
            &sessions,
            silico_app::tab::Edit {
                feature: silico_core::document::FeatureId::new("k7f2qx3m").unwrap(),
                field: "length".to_owned(),
                value: json::read(b"41").unwrap(),
            },
        );
    }
    step_until(h, "the edit applied", |h| {
        h.state().tabs()[0].transaction && !h.state().tabs()[0].unsettled()
    });
    hold
}

#[test]
fn a_refusal_and_a_question_are_let_go_when_the_dialog_opens_again_and_a_quit_cancels_it() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    // A refusal, Cancel, and the dialog again: the refusal isn't said.
    open_export(&mut h);
    set_field(&mut h, "Tolerance", "0");
    press(&mut h, "Export");
    let said = |h: &AppHarness| {
        h.state().tabs()[0]
            .export_said
            .clone()
            .is_some_and(|said| h.query_all_by_label(&said).count() > 0)
    };
    step_until(&mut h, "the refusal said", said);
    let refusal = h.state().tabs()[0].export_said.clone().unwrap();
    press(&mut h, "Cancel");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    open_export(&mut h);
    assert_eq!(h.query_all_by_label(&refusal).count(), 0);
    press(&mut h, "Cancel");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    // A question about a file at the path; a quit asked meanwhile (edits held) cancels the
    // dialog; the quit cancelled, the dialog opens on its form, not the old question.
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let out = path.with_extension("step");
    std::fs::write(&out, "old").unwrap();
    open_export(&mut h);
    press(&mut h, "STEP");
    press(&mut h, "Export");
    let asked = format!("{} already exists. Replace it?", out.display());
    step_until(&mut h, "the question", |h| anywhere(h, &asked));
    close_window(&mut h);
    step_until(&mut h, "the quit's question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    assert!(h.state().tabs()[0].export_replacing.is_none());
    h.query_all_by_role_and_label(Role::Button, "Cancel")
        .last()
        .unwrap()
        .click_accesskit();
    step(&mut h);
    open_export(&mut h);
    assert!(!anywhere(&h, &asked));
    assert!(h.query_by_role_and_label(Role::Button, "Replace").is_none());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "old");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn export_is_off_while_a_feature_dialog_is_open_or_the_tab_closes() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let export_off = |h: &mut AppHarness| {
        h.get_by_label("File").click();
        step(h);
        step(h);
        let off = h
            .get_by_role_and_label(Role::Button, "Export…")
            .accesskit_node()
            .is_disabled();
        h.get_by_label("File").click();
        step(h);
        step(h);
        off
    };
    assert!(!export_off(&mut h));
    // A name typed (so the close asks), then a feature dialog open: off.
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    settled(&mut h);
    create(&mut h, "Box");
    assert!(export_off(&mut h));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    // Closing, its save held: off.
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    step(&mut h);
    assert!(h.state().tabs()[0].closing());
    assert!(export_off(&mut h));
    hold.release();
    check.wait();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_export_again_lets_go_of_the_last_refusal_and_answer() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    open_export(&mut h);
    set_field(&mut h, "Tolerance", "0");
    press(&mut h, "Export");
    step_until(&mut h, "the refusal", |h| {
        h.state().tabs()[0].export_said.is_some()
    });
    // STEP now, its writing held: the refusal gone from the dialog, and no answer yet.
    let hold = f.kernel.hold(is_step);
    press(&mut h, "STEP");
    press(&mut h, "Export");
    assert!(f.kernel.wait_running(1, WAIT));
    step(&mut h);
    assert_eq!(h.state().tabs()[0].export_said, None);
    assert!(h.state().tabs()[0].export_answer.is_none());
    hold.release();
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(path.with_extension("step").is_file());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn only_its_own_tabs_export_closes_a_dialog() {
    use silico_core::command::export::{Export, ExportFormat};
    let f = fixture();
    let (mut h, a) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let b = f.file("other.silico", EXAMPLE);
    h.state_mut().open(b.to_str().unwrap(), false);
    step_until(&mut h, "two tabs", |h| {
        h.state().tabs().len() == 2 && h.state().tabs()[1].timeline.is_some()
    });
    // The first tab's export held, sent without its dialog; the second's dialog open.
    let hold = f.kernel.hold(is_step);
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        tab.export(
            &sessions,
            Export {
                format: ExportFormat::Step,
                path: a.with_extension("step"),
                replace: false,
                features: None,
            },
        );
    }
    assert!(f.kernel.wait_running(1, WAIT));
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Export…").click();
    step_until(&mut h, "the second's dialog", |h| {
        anywhere(h, "Export other.silico")
    });
    hold.release();
    step_until(&mut h, "the first's export written", |h| {
        h.state().tabs()[0].export_answer.is_some()
    });
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(a.with_extension("step").is_file());
    assert!(anywhere(&h, "Export other.silico"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn enter_exports_and_esc_cancels_the_export_dialog() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    open_export(&mut h);
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(h.state().tabs()[0].exports.is_empty());
    open_export(&mut h);
    press(&mut h, "STEP");
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(path.with_extension("step").is_file());
    assert_eq!(h.state().tabs()[0].exports.len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_bare_name_is_written_in_the_documents_folder_for_export_and_save_as() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let folder = path.parent().unwrap();
    open_export(&mut h);
    press(&mut h, "STEP");
    set_field(&mut h, "Path", "bare.step");
    press(&mut h, "Export");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(folder.join("bare.step").is_file());
    assert_eq!(
        h.state().tabs()[0].exports[0].path,
        folder.join("bare.step")
    );
    // Save As, Enter its Save.
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Save As…").click();
    step_until(&mut h, "the Save As dialog", |h| {
        anywhere(h, "Save example.silico as")
    });
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text("bare.silico");
    step(&mut h);
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the tab of the new file", |h| {
        h.state().tabs()[0].path == folder.join("bare.silico").to_str().unwrap()
    });
    assert!(folder.join("bare.silico").is_file());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_cancels_save_as_and_open_and_enter_opens() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Save As…").click();
    step_until(&mut h, "the Save As dialog", |h| {
        anywhere(h, "Save example.silico as")
    });
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "Save As closed", |h| {
        !anywhere(h, "Save example.silico as")
    });
    assert_eq!(h.state().tabs()[0].saves, 0);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Open…").click();
    step_until(&mut h, "the Open dialog", |h| anywhere(h, "Open document"));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "Open closed", |h| !anywhere(h, "Open document"));
    assert_eq!(h.state().tabs().len(), 1);
    // Enter is Open's Open.
    let other = f.file("other.silico", EXAMPLE);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Open…").click();
    step_until(&mut h, "the Open dialog", |h| anywhere(h, "Open document"));
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text(other.to_str().unwrap());
    step(&mut h);
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the second tab", |h| h.state().tabs().len() == 2);
    assert_eq!(never_waits::waits(), 0);
}

// ---- Enter and Esc in the modals (#330's second round) ----

/// File > `item` focused and Enter pressed: its own click.
fn menu_item_by_enter(h: &mut AppHarness, item: &str) {
    h.get_by_label("File").click();
    step(h);
    step(h);
    h.get_by_role_and_label(Role::Button, item).focus();
    step(h);
    h.key_press(eframe::egui::Key::Enter);
    step(h);
    step(h);
}

/// The open modal's Cancel focused and Enter pressed: Cancel's own click, never the OK.
fn cancel_by_enter(h: &mut AppHarness) {
    h.query_all_by_role_and_label(Role::Button, "Cancel")
        .last()
        .unwrap()
        .focus();
    step(h);
    h.key_press(eframe::egui::Key::Enter);
    step(h);
    step(h);
}

#[test]
fn enter_on_a_focused_menu_item_opens_its_dialog_and_does_nothing_more() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // Export…: the dialog open, nothing exported.
    menu_item_by_enter(&mut h, "Export…");
    assert!(exporting_dialog(&h));
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(h.state().tabs()[0].exports.is_empty());
    assert!(!path.with_extension("3mf").exists());
    // Enter on its focused Cancel: cancelled, nothing written.
    cancel_by_enter(&mut h);
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(h.state().tabs()[0].exports.is_empty());
    // Open…, a path left in its field from before: open, nothing opened.
    let other = f.file("other.silico", EXAMPLE);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Open…").click();
    step_until(&mut h, "the Open dialog", |h| anywhere(h, "Open document"));
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text(other.to_str().unwrap());
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    step_until(&mut h, "Open closed", |h| !anywhere(h, "Open document"));
    menu_item_by_enter(&mut h, "Open…");
    assert!(anywhere(&h, "Open document"));
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs().len(), 1);
    cancel_by_enter(&mut h);
    step_until(&mut h, "Open closed", |h| !anywhere(h, "Open document"));
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs().len(), 1);
    // Save As…: likewise, its path typed, then Cancel by Enter.
    menu_item_by_enter(&mut h, "Save As…");
    assert!(anywhere(&h, "Save example.silico as"));
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text("copy.silico");
    step(&mut h);
    cancel_by_enter(&mut h);
    step_until(&mut h, "Save As closed", |h| {
        !anywhere(h, "Save example.silico as")
    });
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert!(!path.with_file_name("copy.silico").exists());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_path_is_sent_as_typed_spaces_and_all() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let folder = path.parent().unwrap().to_owned();
    open_export(&mut h);
    press(&mut h, "STEP");
    set_field(&mut h, "Path", " spaced.step");
    press(&mut h, "Export");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert!(folder.join(" spaced.step").is_file());
    assert!(!folder.join("spaced.step").exists());
    // Save As likewise.
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Save As…").click();
    step_until(&mut h, "the Save As dialog", |h| {
        anywhere(h, "Save example.silico as")
    });
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text(" spaced.silico");
    step(&mut h);
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "saved there", |h| {
        h.state().tabs()[0].path == folder.join(" spaced.silico").to_str().unwrap()
    });
    assert!(folder.join(" spaced.silico").is_file());
    // Open: spaces alone are no path; Enter opens nothing, and nothing is refused.
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Open…").click();
    step_until(&mut h, "the Open dialog", |h| anywhere(h, "Open document"));
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text("   ");
    step(&mut h);
    h.key_press(eframe::egui::Key::Enter);
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(anywhere(&h, "Open document"));
    assert_eq!(h.state().message(), None);
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

// ---- #330's third round ----

#[test]
fn enter_held_on_export_opens_the_dialog_and_its_repeats_export_nothing() {
    use eframe::egui::{Event, Key, Modifiers};
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Export…").focus();
    step(&mut h);
    for repeat in [false, true, true, true, true, true] {
        h.input_mut().events.push(Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
    }
    assert!(exporting_dialog(&h));
    assert!(h.state().tabs()[0].exports.is_empty());
    assert!(!path.with_extension("3mf").exists());
    // Let go, then a fresh press: its OK.
    h.input_mut().events.push(Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    step(&mut h);
    h.key_press(Key::Enter);
    step_until(&mut h, "its answer", |h| {
        h.state().tabs()[0].export_answer.is_some()
    });
    assert_eq!(h.state().tabs()[0].exports.len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_is_no_on_the_replace_close_and_quit_questions() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    // Save As over a file there: asked, and Esc writes nothing.
    let there = path.with_file_name("there.silico");
    std::fs::write(&there, "theirs").unwrap();
    save_as(&mut h, &there);
    let asked = format!("{} already exists. Replace it?", there.display());
    step_until(&mut h, "the replace question", |h| anywhere(h, &asked));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "no question", |h| !anywhere(h, &asked));
    assert_eq!(std::fs::read_to_string(&there).unwrap(), "theirs");
    // Close: asked, and Esc keeps the tab.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::W);
    step_until(&mut h, "the close question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "no question", |h| {
        !anywhere(h, "Save changes to example.silico before closing?")
    });
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    // Quit: asked, and Esc keeps the window and the tab.
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "no question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_none()
    });
    for _ in 0..5 {
        step(&mut h);
        assert!(!told(&h, true));
    }
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn open_and_save_as_each_keep_their_own_path() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let menu = |h: &mut AppHarness, item: &str| {
        h.get_by_label("File").click();
        step(h);
        step(h);
        h.get_by_role_and_label(Role::Button, item).click();
        step(h);
        step(h);
    };
    // Open, a path typed, cancelled.
    menu(&mut h, "Open…");
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text("typed-in-open.silico");
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    step_until(&mut h, "Open closed", |h| !anywhere(h, "Open document"));
    // Save As opens with a field of its own: empty, not Open's.
    menu(&mut h, "Save As…");
    assert_eq!(
        h.get_by_role_and_label(Role::TextInput, "Path")
            .value()
            .as_deref(),
        Some("")
    );
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text("typed-in-save-as.silico");
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    step_until(&mut h, "Save As closed", |h| {
        !anywhere(h, "Save example.silico as")
    });
    // Open again: its own last; Save As again: empty.
    menu(&mut h, "Open…");
    assert_eq!(
        h.get_by_role_and_label(Role::TextInput, "Path")
            .value()
            .as_deref(),
        Some("typed-in-open.silico")
    );
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    step_until(&mut h, "Open closed", |h| !anywhere(h, "Open document"));
    menu(&mut h, "Save As…");
    assert_eq!(
        h.get_by_role_and_label(Role::TextInput, "Path")
            .value()
            .as_deref(),
        Some("")
    );
    // And by Ctrl+Shift+S, after something typed in it: empty again.
    h.get_by_role_and_label(Role::TextInput, "Path").focus();
    step(&mut h);
    h.get_by_role_and_label(Role::TextInput, "Path")
        .type_text("typed-again.silico");
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    step_until(&mut h, "Save As closed", |h| {
        !anywhere(h, "Save example.silico as")
    });
    h.key_press_modifiers(
        eframe::egui::Modifiers::COMMAND | eframe::egui::Modifiers::SHIFT,
        eframe::egui::Key::S,
    );
    step_until(&mut h, "Save As again", |h| {
        anywhere(h, "Save example.silico as")
    });
    assert_eq!(
        h.get_by_role_and_label(Role::TextInput, "Path")
            .value()
            .as_deref(),
        Some("")
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn enter_right_after_a_menu_item_opened_the_dialog_by_enter_is_its_ok() {
    // The focus left on the menu item, no longer drawn, is no focus: a fresh Enter in the
    // very next frame is the dialog's OK.
    use eframe::egui::{Event, Key, Modifiers};
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Export…").focus();
    step(&mut h);
    let enter = |pressed: bool| Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    h.input_mut().events.push(enter(true));
    step(&mut h);
    assert!(exporting_dialog(&h));
    h.input_mut().events.push(enter(false));
    h.input_mut().events.push(enter(true));
    step(&mut h);
    step_until(&mut h, "its answer", |h| {
        h.state().tabs()[0].export_answer.is_some()
    });
    assert_eq!(h.state().tabs()[0].exports.len(), 1);
    assert!(!path.with_extension("step").exists());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn enter_on_a_feature_dialogs_focused_cancel_cancels() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let before = text_of(&f, &h);
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    dialog(&h)
        .get_by_role_and_label(Role::Button, "Cancel")
        .focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::Enter);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_format_chosen_is_the_one_marked_chosen() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let chosen = |h: &AppHarness, word: &str| {
        h.get_by_role_and_label(Role::Button, word)
            .accesskit_node()
            .toggled()
            == Some(eframe::egui::accesskit::Toggled::True)
    };
    open_export(&mut h);
    assert!(chosen(&h, "3MF") && !chosen(&h, "STEP"));
    press(&mut h, "STEP");
    assert!(chosen(&h, "STEP") && !chosen(&h, "3MF"));
    press(&mut h, "3MF");
    assert!(chosen(&h, "3MF") && !chosen(&h, "STEP"));
    assert_eq!(never_waits::waits(), 0);
}

// ---- The viewport (M5.6) ----

/// Two boxes, each a body: a 50 mm cube at the origin, and a 1 m cube 2 m along x. The
/// fake meshes a box at its floor, its box's diagonal × 1e-4 (`mesh_floor`): the small
/// one's 0.00866 mm, under every level these tests view it at; the large one's
/// 1000√3 × 1e-4 = 0.173205 mm, over all of them, so it's meshed at its floor, which
/// serves every half pixel under 2 · 0.125 (its floor's level, 2⁻³).
const TWO_BOXES: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Small", "position": [0, 0, 0], "length": 50, "width": 50, "height": 50},
    {"id": "a91cw0de", "type": "box", "v": 1, "name": "Large", "position": [2000, 0, 0], "length": 1000, "width": 1000, "height": 1000}
  ]
}
"#;

/// What the fake renderer was told: each upload, by body, with its mesh; each forget; each
/// frame; and the picking answer to give once a frame asks for one.
#[derive(Default)]
struct Told {
    uploads: Vec<(u32, Arc<silico_protocol::Mesh>)>,
    forgets: Vec<u32>,
    frames: Vec<silico_app::renderer::FrameData>,
    pick: Option<silico_app::renderer::Pick>,
    /// The pick asked, and the number of the frame that asked it.
    picking: Option<(u64, silico_app::renderer::PixelRect)>,
}

type Record = Arc<Mutex<Told>>;

/// A renderer that needs no GPU: it counts what it's told, as the real one is told it.
struct FakeRenderer(Record);

impl silico_app::renderer::ViewRenderer for FakeRenderer {
    type Failure = String;
    fn upload(&mut self, body: u32, mesh: &Arc<silico_protocol::Mesh>) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .uploads
            .push((body, Arc::clone(mesh)));
        Ok(())
    }
    fn forget(&mut self, body: u32) {
        self.0.lock().unwrap().forgets.push(body);
    }
    fn prepare(
        &mut self,
        _: &silico_app::renderer::Frame<'_>,
    ) -> Result<eframe::wgpu::CommandBuffer, String> {
        panic!("no GPU in these tests")
    }
    fn composite(&self, _: &mut eframe::wgpu::RenderPass<'_>) {}
    fn take_picking(&mut self) -> Option<silico_app::renderer::Picking> {
        let mut told = self.0.lock().unwrap();
        let (frame, rect) = told.picking.take()?;
        Some(silico_app::renderer::Picking {
            frame,
            rect,
            hit: told.pick,
        })
    }
}

/// Frames handed to the fake renderer on the UI thread, its changes made through the same
/// `apply` the GPU's paint callback makes them through, and its picking a frame late.
struct Recorder(FakeRenderer);

impl silico_app::renderer::Present for Recorder {
    fn present(
        &mut self,
        _: &eframe::egui::Painter,
        _: eframe::egui::Rect,
        changes: Vec<silico_app::renderer::Change>,
        frame: Option<silico_app::renderer::FrameData>,
    ) {
        assert!(silico_app::renderer::apply(&mut self.0, changes).is_empty());
        if let Some(frame) = frame {
            let mut told = self.0.0.lock().unwrap();
            // The renderer takes bodies ascending, each once.
            assert!(frame.bodies.windows(2).all(|pair| pair[0] < pair[1]));
            if let Some(rect) = frame.picking {
                told.picking = Some((frame.serial, rect));
            }
            told.frames.push(frame);
        }
    }
    fn picked(&mut self) -> Option<silico_app::renderer::Picking> {
        use silico_app::renderer::ViewRenderer as _;
        self.0.take_picking()
    }
    fn refused(&mut self) -> std::collections::BTreeSet<u32> {
        std::collections::BTreeSet::new()
    }
}

impl Fixture {
    /// The app with `text` open from `name`, drawn through the fake renderer.
    fn drawn(&self, name: &str, text: &str) -> (AppHarness, Record) {
        let path = self.file(name, text);
        let record = Record::default();
        let app = App::new(Arc::clone(&self.sessions))
            .presenting(Box::new(Recorder(FakeRenderer(Arc::clone(&record)))));
        // A sixtieth of a second a frame, as a display's: a double-click's two clicks are
        // two frames apart.
        let mut h = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size((1280.0, 800.0))
                .with_step_dt(1.0 / 60.0)
                .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
        };
        h.state_mut().open(path.to_str().unwrap(), false);
        (h, record)
    }
}

fn view(h: &AppHarness) -> &silico_app::viewport::View {
    &h.state().tabs()[0].view
}

fn view_mut(h: &mut AppHarness) -> &mut silico_app::viewport::View {
    &mut h.state_mut().tabs_mut()[0].view
}

/// How many meshes the fake kernel has made.
fn tessellated(f: &Fixture) -> usize {
    f.kernel
        .calls()
        .iter()
        .filter(|call| matches!(call.op, Op::Tessellate { .. }))
        .count()
}

/// Frames stepped until the tab's read has answered and nothing more is read: its bodies
/// held, of the published revision, no read out, and none wanted over three more frames.
fn meshes_settled(h: &mut AppHarness) {
    step_until(h, "the meshes read", |h| {
        h.state().tabs().first().is_some_and(|tab| {
            !tab.view.reading() && tab.published.is_some() && tab.view.model() == tab.published
        })
    });
    for _ in 0..3 {
        step(h);
    }
    step_until(h, "the meshes read", |h| !view(h).reading());
}

fn uploads(record: &Record) -> Vec<u32> {
    record
        .lock()
        .unwrap()
        .uploads
        .iter()
        .map(|(body, _)| *body)
        .collect()
}

/// The viewport's place in the window, in points, as it was last frame.
fn the_viewport(h: &AppHarness) -> eframe::egui::Rect {
    view(h).frame().unwrap().rect
}

#[track_caller]
fn near(a: [f64; 3], b: [f64; 3], within: f64) {
    for k in 0..3 {
        assert!((a[k] - b[k]).abs() <= within, "{a:?} against {b:?}");
    }
}

fn minus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[test]
fn a_models_buffers_are_uploaded_once_over_a_hundred_frames_and_an_orthographic_orbit_reads_nothing()
 {
    let f = fixture();
    let (mut h, record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    // One body, the cut, as body 1, read once (Home's view, fitted by the core), meshed
    // once, and uploaded once, with the mesh the read gave.
    assert_eq!(view(&h).reads(), 1);
    assert_eq!(tessellated(&f), 1);
    assert_eq!(uploads(&record), [1]);
    let mesh = Arc::clone(&view(&h).bodies()[&1].mesh);
    assert!(Arc::ptr_eq(&record.lock().unwrap().uploads[0].1, &mesh));
    // Home: from the top front right, onto the cut's box's centre, (20, 10, 5).
    let home = view(&h).camera().clone();
    assert_eq!(home.target, [20.0, 10.0, 5.0]);
    let d = minus(home.eye, home.target);
    assert!(d[0] > 0.0 && (d[0] + d[1]).abs() < 1e-6 && (d[0] - d[2]).abs() < 1e-6);
    let framed = record.lock().unwrap().frames.len();
    for _ in 0..100 {
        step(&mut h);
    }
    {
        let told = record.lock().unwrap();
        assert_eq!(told.uploads.len(), 1);
        assert!(told.forgets.is_empty());
        assert!(told.frames.len() >= framed + 100);
        assert!(
            told.frames[framed..]
                .iter()
                .all(|frame| frame.bodies == [1] && frame.camera == home)
        );
    }
    assert_eq!((view(&h).reads(), tessellated(&f)), (1, 1));
    // An orbit, Shift and the middle button dragged 90 points right, ten at a time: 45°
    // about z through the target. The eye's way from the target, (1, −1, 1)/√3, turned
    // by −45°, is (0, −√2, 1)/√3: the front, from above. The half pixel of an
    // orthographic view doesn't change as it turns, so nothing is read or uploaded.
    let at = the_viewport(&h).center() - eframe::egui::vec2(100.0, 0.0);
    let shift = eframe::egui::Modifiers::SHIFT;
    h.event(eframe::egui::Event::ModifiersChanged(shift));
    h.hover_at(at);
    h.event(eframe::egui::Event::PointerButton {
        pos: at,
        button: eframe::egui::PointerButton::Middle,
        pressed: true,
        modifiers: shift,
    });
    step(&mut h);
    for i in 1..=9u8 {
        h.hover_at(at + eframe::egui::vec2(10.0 * f32::from(i), 0.0));
        step(&mut h);
    }
    h.event(eframe::egui::Event::PointerButton {
        pos: at + eframe::egui::vec2(90.0, 0.0),
        button: eframe::egui::PointerButton::Middle,
        pressed: false,
        modifiers: shift,
    });
    h.event(eframe::egui::Event::ModifiersChanged(
        eframe::egui::Modifiers::NONE,
    ));
    step(&mut h);
    for _ in 0..10 {
        step(&mut h);
    }
    let turned = view(&h).camera().clone();
    assert_eq!(turned.target, home.target);
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let way = [0.0, -(2f64.sqrt()), 1.0].map(|c| c / 3f64.sqrt() * distance);
    near(minus(turned.eye, turned.target), way, 1e-9 * distance);
    assert_eq!(turned.projection, home.projection);
    assert_eq!((view(&h).reads(), tessellated(&f)), (1, 1));
    assert_eq!(uploads(&record), [1]);
    assert_eq!(record.lock().unwrap().frames.last().unwrap().camera, turned);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_zoom_past_serves_reads_again_listing_what_is_held_and_uploads_only_the_body_whose_level_changed()
 {
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    assert_eq!(uploads(&record), [1, 2]);
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1, 2]);
    // From the front onto the small cube, at half a pixel of 0.04 mm (the view 0.08 mm a
    // pixel high): the small cube's level is 2⁻⁵; the large one is at its floor.
    let height = view(&h).frame().unwrap().size_px[1];
    let camera = silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic {
            view_height: 0.08 * f64::from(height),
        },
        eye: [25.0, -500.0, 25.0],
        target: [25.0, 25.0, 25.0],
        up: [0.0, 0.0, 1.0],
    };
    view_mut(&mut h).set_camera(camera);
    meshes_settled(&mut h);
    let floor = 1000.0 * 3f64.sqrt() * 1e-4;
    let at = |h: &AppHarness, n: u32| {
        let body = &view(h).bodies()[&n];
        (body.level, body.deflection, body.serves)
    };
    assert_eq!(view(&h).reads(), 2);
    assert_eq!(at(&h, 1), (0.03125, 0.03125, [0.03125, 0.0625]));
    let (level, deflection, serves) = at(&h, 2);
    assert_eq!((level, serves), (0.03125, [0.0, 0.25]));
    assert!((deflection - floor).abs() < 1e-12, "{deflection}");
    let (made, told) = (tessellated(&f), uploads(&record).len());
    // The wheel, 200 points toward the model at the view's centre: the view halves, so
    // the half pixel is 0.02, under the small cube's [0.03125, 0.0625); the large one's
    // [0, 0.25) still serves it. One read, holding both; the small cube meshed at 2⁻⁶
    // and uploaded again, alone; the large one answered held.
    // The read held: meanwhile each frame draws the bodies held, under the new camera.
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    h.hover_at(the_viewport(&h).center());
    h.event(eframe::egui::Event::MouseWheel {
        unit: eframe::egui::MouseWheelUnit::Point,
        delta: eframe::egui::vec2(0.0, 200.0),
        phase: eframe::egui::TouchPhase::Move,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    step(&mut h);
    match view(&h).camera().projection {
        silico_protocol::Projection::Orthographic { view_height } => {
            assert_eq!(view_height, 0.04 * f64::from(height));
        }
        silico_protocol::Projection::Perspective { .. } => panic!("orthographic"),
    }
    step_until(&mut h, "the zoom's read held", |h| view(h).reading());
    for _ in 0..5 {
        step(&mut h);
    }
    {
        let told = record.lock().unwrap();
        let last = told.frames.last().unwrap();
        assert_eq!(last.bodies, [1, 2]);
        assert_eq!(&last.camera, view(&h).camera());
    }
    hold.release();
    meshes_settled(&mut h);
    assert_eq!(view(&h).reads(), 3);
    assert_eq!(at(&h, 1), (0.015625, 0.015625, [0.015625, 0.03125]));
    assert_eq!(at(&h, 2).1, deflection);
    // The read held both: the large cube was answered held (the key and deflection the tab
    // sent were its own), the small one, at a new level, with arrays.
    assert!(view(&h).bodies()[&2].held);
    assert!(!view(&h).bodies()[&1].held);
    assert_eq!(tessellated(&f), made + 1);
    assert_eq!(uploads(&record)[told..], [1]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn frames_climb_and_the_panels_answer_while_the_meshes_read_is_held() {
    let f = fixture();
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let (mut h, record) = f.drawn("example.silico", EXAMPLE);
    step_until(&mut h, "the meshes read held", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.view.reading() && tab.timeline.is_some())
    });
    let frames = h.state().frames();
    for _ in 0..30 {
        step(&mut h);
    }
    assert!(h.state().frames() >= frames + 30);
    assert!(view(&h).reading());
    assert_eq!(view(&h).reads(), 1);
    assert!(shows(&h, "Timeline", "Hole"));
    select(&mut h, "Pin");
    assert!(uploads(&record).is_empty());
    hold.release();
    step_until(&mut h, "the body uploaded", |h| !view(h).reading());
    step(&mut h);
    assert_eq!(uploads(&record), [1]);
    assert_eq!(view(&h).reads(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edits_new_revision_reads_again_and_a_body_gone_is_forgotten() {
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    assert_eq!(uploads(&record), [1, 2]);
    let id = h.state().tabs()[0].id;
    let opened = h.state().tabs()[0].published;
    let keys = |h: &AppHarness| -> Vec<String> {
        view(h)
            .bodies()
            .values()
            .map(|body| body.key.to_string())
            .collect()
    };
    let before = keys(&h);
    let made = tessellated(&f);
    // Another client lengthens the small cube: a new revision, whose model is read with
    // both meshes held; the small cube's key changes, so it alone is uploaded again.
    let small = silico_core::document::FeatureId::new("k7f2qx3m").unwrap();
    f.sessions
        .set_field(
            &id,
            h.state().tabs()[0].revision,
            small,
            "length".to_owned(),
            json::read(b"60").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the edit's model read", |h| {
        view(h).model() > opened && !view(h).reading()
    });
    meshes_settled(&mut h);
    let after = keys(&h);
    assert_ne!(after[0], before[0]);
    assert_eq!(after[1], before[1]);
    // Only the small cube meshed: the large one was held, at its key and deflection, and
    // answered so.
    assert_eq!(tessellated(&f), made + 1);
    assert!(view(&h).bodies()[&2].held);
    assert!(!view(&h).bodies()[&1].held);
    assert_eq!(uploads(&record), [1, 2, 1]);
    assert!(record.lock().unwrap().forgets.is_empty());
    // The large cube removed: body 2 forgotten, nothing uploaded.
    let large = silico_core::document::FeatureId::new("a91cw0de").unwrap();
    let revision = f.sessions.status(&id).session.unwrap().revision;
    f.sessions
        .remove_feature(&id, revision, large)
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "body 2 forgotten", |h| {
        !view(h).bodies().contains_key(&2)
    });
    step(&mut h);
    assert_eq!(record.lock().unwrap().forgets, [2]);
    assert_eq!(uploads(&record), [1, 2, 1]);
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_view_cubes_front_is_the_cores_front_view_and_f6_fits_after_a_pan() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    h.get_by_role_and_label(Role::Group, "ViewCube")
        .get_by_label("Front")
        .click();
    step(&mut h);
    // From −y onto the cut's box's centre, z up: the box 40 across and 12 high, 1.1 times
    // the larger of 12 and 40 · height / width.
    let [width, height] = view(&h).frame().unwrap().size_px;
    let fitted = 1.1 * 12f64.max(40.0 * f64::from(height) / f64::from(width));
    let front = view(&h).camera().clone();
    assert_eq!(front.target, [20.0, 10.0, 5.0]);
    let d = minus(front.eye, front.target);
    assert!(d[0] == 0.0 && d[1] < 0.0 && d[2] == 0.0, "{d:?}");
    assert_eq!(front.up, [0.0, 0.0, 1.0]);
    let view_height = |camera: &silico_protocol::Camera| match camera.projection {
        silico_protocol::Projection::Orthographic { view_height } => view_height,
        silico_protocol::Projection::Perspective { .. } => panic!("orthographic"),
    };
    assert!((view_height(&front) - fitted).abs() < 1e-9);
    // The middle button pans 40 points left: the target moves right by 40 points' worth.
    let at = the_viewport(&h).center() - eframe::egui::vec2(100.0, 0.0);
    h.hover_at(at);
    h.event(eframe::egui::Event::PointerButton {
        pos: at,
        button: eframe::egui::PointerButton::Middle,
        pressed: true,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    h.hover_at(at - eframe::egui::vec2(40.0, 0.0));
    h.event(eframe::egui::Event::PointerButton {
        pos: at - eframe::egui::vec2(40.0, 0.0),
        button: eframe::egui::PointerButton::Middle,
        pressed: false,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    step(&mut h);
    let points = f64::from(the_viewport(&h).height());
    let panned = view(&h).camera().clone();
    near(
        panned.target,
        [20.0 + 40.0 * view_height(&front) / points, 10.0, 5.0],
        1e-9,
    );
    // F6 fits again: the box's centre, as high as the ViewCube's fit.
    h.key_press(eframe::egui::Key::F6);
    step(&mut h);
    let refit = view(&h).camera().clone();
    near(refit.target, [20.0, 10.0, 5.0], 1e-12);
    assert!((view_height(&refit) - fitted).abs() < 1e-9);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn each_frame_gives_the_camera_its_size_scale_pointer_highlights_and_picking_a_frame_late() {
    let f = fixture();
    let (mut h, record) = f.drawn("example.silico", EXAMPLE);
    h.set_pixels_per_point(2.0);
    meshes_settled(&mut h);
    let rect = the_viewport(&h);
    let at = rect.min + eframe::egui::vec2(30.0, 40.0);
    h.hover_at(at);
    step(&mut h);
    let frame = view(&h).frame().unwrap().clone();
    assert_eq!(frame.pixels_per_point, 2.0);
    assert_eq!(frame.pointer_px, Some([60.0, 80.0]));
    let size = [rect.width() * 2.0, rect.height() * 2.0].map(|side| side.round());
    assert_eq!(frame.size_px.map(|side| side as f32), size);
    assert_eq!(&frame.camera, view(&h).camera());
    // The selection's highlights are passed through as they're set.
    let face = (1, silico_app::renderer::Element::Face(3));
    let edge = (1, silico_app::renderer::Element::Edge(2));
    view_mut(&mut h).highlights = [face, edge].into_iter().collect();
    // A picking rectangle asked: in the next frame only, and its result the frame after.
    let rect_px = silico_app::renderer::PixelRect {
        min: [58, 78],
        size: [5, 5],
    };
    let pick = silico_app::renderer::Pick {
        body: 1,
        hit: silico_app::renderer::Element::Face(3),
        pixel: [60, 80],
        point: Some([20.0, 0.0, 5.0]),
    };
    record.lock().unwrap().pick = Some(pick);
    // A rectangle past what a renderer takes isn't asked.
    let wide = silico_app::renderer::PixelRect {
        min: [0, 0],
        size: [257, 1],
    };
    assert!(!view_mut(&mut h).pick(wide));
    assert!(view_mut(&mut h).pick(rect_px));
    step(&mut h);
    let serial = {
        let told = record.lock().unwrap();
        let last = told.frames.last().unwrap();
        assert_eq!(last.highlights, [face, edge]);
        assert_eq!(last.picking, Some(rect_px));
        last.serial
    };
    assert_eq!(view(&h).picked(), None);
    step(&mut h);
    assert_eq!(
        view(&h).picked(),
        Some(silico_app::renderer::Picking {
            frame: serial,
            rect: rect_px,
            hit: Some(pick)
        })
    );
    assert_eq!(record.lock().unwrap().frames.last().unwrap().picking, None);
    // A new pick asked lets the last answer go at once.
    assert!(view_mut(&mut h).pick(rect_px));
    assert_eq!(view(&h).picked(), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_body_hidden_in_the_browser_is_left_out_of_the_frame_and_kept_uploaded() {
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    step_until(&mut h, "the browser's bodies", |h| {
        shows(h, "Browser", "Hide Large")
    });
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Hide Large")
        .click();
    step(&mut h);
    step(&mut h);
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1]);
    assert_eq!(uploads(&record), [1, 2]);
    assert!(record.lock().unwrap().forgets.is_empty());
    // Shown again: drawn again, with nothing uploaded.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Show Large")
        .click();
    step(&mut h);
    step(&mut h);
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1, 2]);
    assert_eq!(uploads(&record), [1, 2]);
    assert_eq!(never_waits::waits(), 0);
}

/// A wheel of `delta` in `unit`s at `at`.
fn wheel(
    h: &mut AppHarness,
    at: eframe::egui::Pos2,
    unit: eframe::egui::MouseWheelUnit,
    delta: f32,
) {
    h.hover_at(at);
    h.event(eframe::egui::Event::MouseWheel {
        unit,
        delta: eframe::egui::vec2(0.0, delta),
        phase: eframe::egui::TouchPhase::Move,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    step(h);
}

fn ortho_height(camera: &silico_protocol::Camera) -> f64 {
    match camera.projection {
        silico_protocol::Projection::Orthographic { view_height } => view_height,
        silico_protocol::Projection::Perspective { .. } => panic!("orthographic"),
    }
}

/// From −y onto the cut's box's centre, (20, 10, 5), 80 mm high.
fn the_cuts_front() -> silico_protocol::Camera {
    silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic { view_height: 80.0 },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

/// `button` pressed at `from`, moved to `to` in `steps` even steps, a frame each, then let
/// go there.
fn drag(
    h: &mut AppHarness,
    button: eframe::egui::PointerButton,
    from: eframe::egui::Pos2,
    to: eframe::egui::Pos2,
    steps: u16,
) {
    h.hover_at(from);
    h.event(eframe::egui::Event::PointerButton {
        pos: from,
        button,
        pressed: true,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    step(h);
    for i in 1..=steps {
        h.hover_at(from + (to - from) * f32::from(i) / f32::from(steps));
        step(h);
    }
    h.event(eframe::egui::Event::PointerButton {
        pos: to,
        button,
        pressed: false,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    step(h);
}

#[test]
fn the_wheel_zooms_toward_the_cursor_by_points_lines_and_pages() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(the_cuts_front());
    step(&mut h);
    let rect = the_viewport(&h);
    let mm = 80.0 / f64::from(rect.height());
    // 200 points at 100 right of the centre and 50 up: halved about the point under it,
    // (20 + 100 mm, 10, 5 + 50 mm), so the target moves half way there.
    wheel(
        &mut h,
        rect.center() + eframe::egui::vec2(100.0, -50.0),
        eframe::egui::MouseWheelUnit::Point,
        200.0,
    );
    let zoomed = view(&h).camera().clone();
    assert!((ortho_height(&zoomed) - 40.0).abs() < 1e-12);
    let expected = [20.0 + 50.0 * mm, 10.0, 5.0 + 25.0 * mm];
    for (got, expected) in zoomed.target.iter().zip(expected) {
        assert!((got - expected).abs() < 1e-9, "{zoomed:?}");
    }
    // Five lines, at egui's 40 points a line: 200 points, halved again, at the centre.
    wheel(
        &mut h,
        rect.center(),
        eframe::egui::MouseWheelUnit::Line,
        5.0,
    );
    assert!((ortho_height(view(&h).camera()) - 20.0).abs() < 1e-12);
    // A page back out: the viewport's height in points, 2^(height / 200) as large.
    wheel(
        &mut h,
        rect.center(),
        eframe::egui::MouseWheelUnit::Page,
        -1.0,
    );
    let paged = 20.0 * libm::exp2(f64::from(rect.height()) / 200.0);
    assert!((ortho_height(view(&h).camera()) - paged).abs() < 1e-9);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_zoom_tool_drag_reads_nothing_while_held_and_once_when_let_go() {
    // The Zoom tool: a left drag up zooms in, 200 points halving the view. Dragged up 400
    // points in four moves, twenty idle frames between them, the button held throughout:
    // no read while it's held, however long the view rests; one read once it's let go.
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(the_cuts_front());
    meshes_settled(&mut h);
    let reads = view(&h).reads();
    h.get_by_role_and_label(Role::Toolbar, "Navigation bar")
        .get_by_label("Zoom")
        .click();
    step(&mut h);
    let at = the_viewport(&h).center() - eframe::egui::vec2(100.0, -100.0);
    h.hover_at(at);
    h.event(eframe::egui::Event::PointerButton {
        pos: at,
        button: eframe::egui::PointerButton::Primary,
        pressed: true,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    step(&mut h);
    for i in 1..=4u8 {
        h.hover_at(at - eframe::egui::vec2(0.0, 100.0 * f32::from(i)));
        for _ in 0..20 {
            step(&mut h);
        }
        assert_eq!(view(&h).reads(), reads, "move {i}");
    }
    assert!((ortho_height(view(&h).camera()) - 20.0).abs() < 1e-9);
    h.event(eframe::egui::Event::PointerButton {
        pos: at - eframe::egui::vec2(0.0, 400.0),
        button: eframe::egui::PointerButton::Primary,
        pressed: false,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    meshes_settled(&mut h);
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(view(&h).reads(), reads + 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_navigation_bars_tools_orbit_pan_and_zoom_a_left_drag_and_fit_fits() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(the_cuts_front());
    step(&mut h);
    let bar = |h: &AppHarness, word: &str| {
        h.get_by_role_and_label(Role::Toolbar, "Navigation bar")
            .get_by_label(word)
            .click();
    };
    let at = the_viewport(&h).center() - eframe::egui::vec2(150.0, -80.0);
    let left = eframe::egui::PointerButton::Primary;
    // No tool: a left drag is the selection's, and moves nothing.
    drag(&mut h, left, at, at + eframe::egui::vec2(60.0, 0.0), 3);
    assert_eq!(view(&h).camera(), &the_cuts_front());
    // Orbit: 180 points right, a quarter turn about z: the eye from −y to −x of the target.
    bar(&h, "Orbit");
    step(&mut h);
    drag(&mut h, left, at, at + eframe::egui::vec2(180.0, 0.0), 6);
    let turned = view(&h).camera().clone();
    let way = [
        turned.eye[0] - turned.target[0],
        turned.eye[1] - turned.target[1],
        turned.eye[2] - turned.target[2],
    ];
    for (k, expected) in [-110.0, 0.0, 0.0].into_iter().enumerate() {
        assert!((way[k] - expected).abs() < 1e-9, "{turned:?}");
    }
    // Orbit again lets the tool go: a left drag moves nothing.
    bar(&h, "Orbit");
    step(&mut h);
    drag(&mut h, left, at, at + eframe::egui::vec2(60.0, 0.0), 3);
    assert_eq!(view(&h).camera(), &turned);
    // Pan, from the front again: 40 points right moves the target 40 points' worth left.
    view_mut(&mut h).set_camera(the_cuts_front());
    bar(&h, "Pan");
    step(&mut h);
    drag(&mut h, left, at, at + eframe::egui::vec2(40.0, 0.0), 4);
    let mm = 80.0 / f64::from(the_viewport(&h).height());
    let panned = view(&h).camera().clone();
    assert!(
        (panned.target[0] - (20.0 - 40.0 * mm)).abs() < 1e-9,
        "{panned:?}"
    );
    // Zoom: 200 points up halves it.
    bar(&h, "Zoom");
    step(&mut h);
    drag(&mut h, left, at, at - eframe::egui::vec2(0.0, 200.0), 4);
    assert!((ortho_height(view(&h).camera()) - 40.0).abs() < 1e-9);
    // Fit: the cut's box's centre, 1.1 times the larger of its 12 mm height and its 40 mm
    // across times the view's height over its width.
    bar(&h, "Fit");
    step(&mut h);
    let [width, height] = view(&h).frame().unwrap().size_px;
    let fitted = 1.1 * 12f64.max(40.0 * f64::from(height) / f64::from(width));
    let fit = view(&h).camera().clone();
    assert_eq!(fit.target, [20.0, 10.0, 5.0]);
    assert!((ortho_height(&fit) - fitted).abs() < 1e-9);
    assert_eq!(never_waits::waits(), 0);
}

/// A unit vector's parts within 1e-9 of `expected`, as `camera` looks from its target.
#[track_caller]
fn looks_from(camera: &silico_protocol::Camera, expected: [f64; 3]) {
    let way = [
        camera.eye[0] - camera.target[0],
        camera.eye[1] - camera.target[1],
        camera.eye[2] - camera.target[2],
    ];
    let length = (way[0] * way[0] + way[1] * way[1] + way[2] * way[2]).sqrt();
    for k in 0..3 {
        assert!(
            (way[k] / length - expected[k]).abs() < 1e-9,
            "{camera:?} against {expected:?}"
        );
    }
}

#[test]
fn the_view_cubes_corner_home_drag_and_projection_menu() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    let third = 1.0 / 3f64.sqrt();
    let cube = |h: &AppHarness, label: &str| {
        h.get_by_role_and_label(Role::Group, "ViewCube")
            .get_by_label(label)
            .click();
    };
    // A corner: its isometric view, from (−1, −1, 1)/√3, up (1, 1, 2)/√6, onto the cut's
    // box's centre.
    cube(&h, "Top front left");
    step(&mut h);
    let corner = view(&h).camera().clone();
    looks_from(&corner, [-third, -third, third]);
    assert_eq!(corner.target, [20.0, 10.0, 5.0]);
    let six = 6f64.sqrt();
    for (k, expected) in [1.0 / six, 1.0 / six, 2.0 / six].into_iter().enumerate() {
        assert!((corner.up[k] - expected).abs() < 1e-12);
    }
    // Home: from (1, −1, 1)/√3 again.
    h.get_by_role_and_label(Role::Pane, "Viewport")
        .get_by_label("Home")
        .click();
    step(&mut h);
    let home = view(&h).camera().clone();
    looks_from(&home, [third, -third, third]);
    // A left drag of the cube, 60 points right in one move: a turn of 30° about z, so the
    // way (1, −1, 1)/√3 turns to (cos 30° − sin 30°, −sin 30° − cos 30°, 1)/√3.
    let centre = silico_app::viewport::view_cube_rect(the_viewport(&h)).center();
    let left = eframe::egui::PointerButton::Primary;
    drag(
        &mut h,
        left,
        centre,
        centre + eframe::egui::vec2(60.0, 0.0),
        1,
    );
    let (sin, cos) = (0.5, 3f64.sqrt() / 2.0);
    looks_from(
        view(&h).camera(),
        [(cos - sin) * third, (-sin - cos) * third, third],
    );
    // Its menu: Perspective, at 30°, the eye as far back as shows Home's height at the
    // target; Home then keeps the perspective.
    h.get_by_role_and_label(Role::Pane, "Viewport")
        .get_by_label("Home")
        .click();
    step(&mut h);
    let height = ortho_height(view(&h).camera());
    h.get_by_label("Orbit the view").click_secondary();
    step(&mut h);
    h.get_by_label("Perspective").click_accesskit();
    step(&mut h);
    let perspective = view(&h).camera().clone();
    assert!(view(&h).perspective());
    assert_eq!(
        perspective.projection,
        silico_protocol::Projection::Perspective {
            field_of_view: 30.0
        }
    );
    let back = height / (2.0 * libm::tan(15f64.to_radians()));
    let way = [
        perspective.eye[0] - perspective.target[0],
        perspective.eye[1] - perspective.target[1],
        perspective.eye[2] - perspective.target[2],
    ];
    assert!(((way[0] * way[0] + way[1] * way[1] + way[2] * way[2]).sqrt() - back).abs() < 1e-9);
    cube(&h, "Front");
    step(&mut h);
    assert!(matches!(
        view(&h).camera().projection,
        silico_protocol::Projection::Perspective { .. }
    ));
    looks_from(view(&h).camera(), [0.0, -1.0, 0.0]);
    // Orthographic again.
    h.get_by_label("Orbit the view").click_secondary();
    step(&mut h);
    h.get_by_label("Orthographic").click_accesskit();
    step(&mut h);
    assert!(!view(&h).perspective());
    assert!(matches!(
        view(&h).camera().projection,
        silico_protocol::Projection::Orthographic { .. }
    ));
    assert_eq!(never_waits::waits(), 0);
}

/// The ViewCube's buttons by label, each with its rectangle.
fn cube_buttons(h: &AppHarness) -> std::collections::BTreeMap<String, eframe::egui::Rect> {
    h.get_by_role_and_label(Role::Group, "ViewCube")
        .query_all_by_role(Role::Button)
        .filter_map(|node| Some((node.accesskit_node().label()?, node.rect())))
        .collect()
}

/// The ViewCube's faces as drawn last frame: each polygon's corners and fill, in order.
fn cube_faces(h: &AppHarness) -> Vec<(Vec<eframe::egui::Pos2>, eframe::egui::Color32)> {
    let cube = silico_app::viewport::view_cube_rect(the_viewport(h));
    h.output()
        .shapes
        .iter()
        .filter(|clipped| clipped.clip_rect == cube)
        .filter_map(|clipped| match &clipped.shape {
            eframe::egui::Shape::Path(path) => Some((path.points.clone(), path.fill)),
            _ => None,
        })
        .collect()
}

#[track_caller]
fn at_point(got: eframe::egui::Pos2, expected: eframe::egui::Pos2, what: &str) {
    assert!(
        (got - expected).length() < 1e-3,
        "{what}: {got:?} against {expected:?}"
    );
}

#[test]
fn the_view_cubes_faces_corners_and_home_are_where_the_home_view_sees_them() {
    // From (1, −1, 1) onto the cut's box's centre, z up: the view's right is (1, 1, 0)/√2
    // and its up (−1, 1, 2)/√6, so the cube's point (x, y, z), its faces at ±1, is drawn
    // at ((x + y)/√2, −(−x + y + 2z)/√6) · s from the cube's centre, y down, s half its
    // edge, 0.28 of its 92-point side (the prototype's `CUBE`): 0.28 · 92 = 25.76, so
    // s/√2 = 18.215, s/√6 = 10.516, s·√2 = 36.430, 2s/√6 = 21.033, and 4s/√6 = 42.066
    // (the front's label at (−18.215, 10.516) from the centre, the right's at (18.215,
    // 10.516), the top's at (0, −21.033); the top front left corner at (−36.430, −21.033),
    // the top back right at (36.430, −21.033), the bottom front right at (0, 42.066), the
    // top back left at (0, −42.066), and the bottom front left and back right at
    // (∓36.430, 21.033)). The front, right, and top face the view, each at
    // 1/√3 (over a quarter, so each is labelled where its centre is drawn), shaded 0.6 of
    // the fill edge-on rising to all of it square-on: 0.6 + 0.4/√3. The corners facing it
    // more than a tenth, 1 or 1/3, are clickable at theirs: the top front right (at the
    // cube's centre), the top front left, the top back right, and the bottom front right.
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic { view_height: 80.0 },
        eye: [120.0, -90.0, 105.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    });
    step(&mut h);
    step(&mut h);
    let cube = silico_app::viewport::view_cube_rect(the_viewport(&h));
    let s = 0.28 * 92.0;
    let (r2, r6) = (2f32.sqrt(), 6f32.sqrt());
    let at = |x: f32, y: f32| cube.center() + eframe::egui::vec2(x, y) * s;
    // The cube's corners as drawn: (1, −1, 1) at the centre; (−1, −1, 1), (1, 1, 1),
    // (−1, 1, 1) above; (1, −1, −1) below; (−1, −1, −1) and (1, 1, −1) at the sides.
    let top_front_right = at(0.0, 0.0);
    let top_front_left = at(-r2, -2.0 / r6);
    let top_back_right = at(r2, -2.0 / r6);
    let top_back_left = at(0.0, -4.0 / r6);
    let bottom_front_right = at(0.0, 4.0 / r6);
    let bottom_front_left = at(-r2, 2.0 / r6);
    let bottom_back_right = at(r2, 2.0 / r6);
    let buttons = cube_buttons(&h);
    // Home: 44 by 18 points at the cube's top left, in the 20-point row above it.
    let home = eframe::egui::Rect::from_min_size(
        cube.left_top() - eframe::egui::vec2(0.0, 20.0),
        eframe::egui::vec2(44.0, 18.0),
    );
    let expected = [
        ("Front", at(-1.0 / r2, 1.0 / r6)),
        ("Right", at(1.0 / r2, 1.0 / r6)),
        ("Top", at(0.0, -2.0 / r6)),
        ("Top front right", top_front_right),
        ("Top front left", top_front_left),
        ("Top back right", top_back_right),
        ("Bottom front right", bottom_front_right),
        ("Home", home.center()),
    ];
    let mut labels: Vec<&str> = expected.iter().map(|(label, _)| *label).collect();
    labels.sort_unstable();
    assert_eq!(
        buttons.keys().map(String::as_str).collect::<Vec<_>>(),
        labels
    );
    for (label, centre) in expected {
        at_point(buttons[label].center(), centre, label);
    }
    at_point(buttons["Home"].min, home.min, "Home's corner");
    at_point(buttons["Home"].max, home.max, "Home's far corner");
    let fill = h.ctx.global_style().visuals.widgets.inactive.bg_fill;
    let shade = fill.gamma_multiply(0.6 + 0.4 * (1.0 / 3f64.sqrt()) as f32);
    assert_ne!(shade, fill);
    let faces = cube_faces(&h);
    let expected = [
        (
            "front",
            [
                top_front_right,
                top_front_left,
                bottom_front_left,
                bottom_front_right,
            ],
        ),
        (
            "right",
            [
                top_back_right,
                bottom_back_right,
                bottom_front_right,
                top_front_right,
            ],
        ),
        (
            "top",
            [
                top_back_right,
                top_front_right,
                top_front_left,
                top_back_left,
            ],
        ),
    ];
    assert_eq!(faces.len(), expected.len(), "{faces:?}");
    for ((points, colour), (face, corners)) in faces.iter().zip(expected) {
        assert_eq!(points.len(), 4, "{face}");
        for (got, corner) in points.iter().zip(corners) {
            at_point(*got, corner, face);
        }
        assert_eq!(*colour, shade, "{face}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_face_turned_a_quarter_toward_the_view_is_drawn_and_left_unlabelled() {
    // From (0, −√15, 1) onto the origin: the view's back is (0, −√15, 1)/4, so the top
    // faces it at exactly a quarter (as f64 rounds it, checked), drawn but too oblique to
    // label or click; the front at √15/4, labelled. The left and right are edge-on and the
    // back and bottom face away, none drawn. The four front corners face it more than a
    // tenth, (√15 ± 1)/(4√3); the back ones face away.
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    let camera = silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic { view_height: 80.0 },
        eye: [0.0, -15f64.sqrt(), 1.0],
        target: [0.0; 3],
        up: [0.0, 0.0, 1.0],
    };
    assert_eq!(camera.frame().unwrap().back[2], 0.25);
    view_mut(&mut h).set_camera(camera);
    step(&mut h);
    step(&mut h);
    assert_eq!(
        cube_buttons(&h)
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "Bottom front left",
            "Bottom front right",
            "Front",
            "Home",
            "Top front left",
            "Top front right"
        ]
    );
    assert_eq!(cube_faces(&h).len(), 2);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_view_cubes_menu_marks_the_projection_the_view_has() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    let marked = |h: &AppHarness, word: &str| {
        h.get_by_role_and_label(Role::RadioButton, word)
            .accesskit_node()
            .toggled()
            == Some(eframe::egui::accesskit::Toggled::True)
    };
    // Orthographic, as a view opens.
    h.get_by_label("Orbit the view").click_secondary();
    step(&mut h);
    assert!(marked(&h, "Orthographic") && !marked(&h, "Perspective"));
    h.get_by_label("Perspective").click_accesskit();
    step(&mut h);
    assert!(view(&h).perspective());
    // Perspective once chosen.
    h.get_by_label("Orbit the view").click_secondary();
    step(&mut h);
    assert!(marked(&h, "Perspective") && !marked(&h, "Orthographic"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_navigation_bar_floats_its_margin_above_the_canvass_bottom_centre() {
    // 220 by 32 points (the prototype's `.nav-bar`: a 24-point button, 3 of padding and a
    // 1 border above and below, 24 + 2·3 + 2·1 = 32) about the canvas's middle, its bottom
    // 12 points (`bottom: 12px`) above the canvas's: its middle 12 + 32/2 = 28 points up.
    // Its tools sit in a row along that middle.
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    let canvas = the_viewport(&h);
    let bar = eframe::egui::Rect::from_center_size(
        eframe::egui::pos2(canvas.center().x, canvas.bottom() - 28.0),
        eframe::egui::vec2(220.0, 32.0),
    );
    for word in ["Orbit", "Pan", "Zoom", "Fit"] {
        let tool = h
            .get_by_role_and_label(Role::Toolbar, "Navigation bar")
            .get_by_label(word)
            .rect();
        assert!(bar.contains_rect(tool), "{word}: {tool:?} outside {bar:?}");
        assert!(
            (tool.center().y - bar.center().y).abs() <= 1.0,
            "{word}: {tool:?} off {bar:?}'s middle"
        );
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_middle_double_click_fits() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(silico_protocol::Camera {
        target: [300.0, 10.0, 5.0],
        eye: [300.0, -100.0, 5.0],
        ..the_cuts_front()
    });
    step(&mut h);
    let at = the_viewport(&h).center() - eframe::egui::vec2(100.0, -60.0);
    let middle = eframe::egui::PointerButton::Middle;
    h.hover_at(at);
    step(&mut h);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(eframe::egui::Event::PointerButton {
                pos: at,
                button: middle,
                pressed,
                modifiers: eframe::egui::Modifiers::NONE,
            });
        }
    }
    h.hover_at(at);
    step(&mut h);
    step(&mut h);
    assert_eq!(view(&h).camera().target, [20.0, 10.0, 5.0]);
    assert_eq!(never_waits::waits(), 0);
}

/// A block, 20 mm a side.
const UNMEASURED: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 20, "width": 20, "height": 20}
  ]
}
"#;

/// Every summary answered with no box, so no body can be measured.
fn unmeasured(f: &Fixture) {
    f.kernel.answer_instead(|op| {
        matches!(op, Op::Summarize { .. }).then(|| {
            silico_protocol::Reply::Summary(silico_protocol::ShapeSummary {
                volume: 8000.0,
                area: 2400.0,
                center: [10.0; 3],
                faces: 6,
                edges: 12,
                vertices: 8,
                bounds: None,
                valid: true,
            })
        })
    });
}

#[test]
fn a_model_whose_body_cant_be_measured_opens_at_the_cube_home_and_still_reads() {
    // Every summary answered with no box: Home's view, which fits the bodies' boxes, is
    // refused, naming `view`; the tab keeps its own camera, Home of a 100 mm cube about
    // the origin, and reads with it.
    let f = fixture();
    unmeasured(&f);
    let (mut h, record) = f.drawn("unmeasured.silico", UNMEASURED);
    meshes_settled(&mut h);
    assert_eq!(view(&h).reads(), 2);
    let camera = view(&h).camera().clone();
    assert_eq!(camera.target, [0.0; 3]);
    let third = 1.0 / 3f64.sqrt();
    looks_from(&camera, [third, -third, third]);
    assert_eq!(view(&h).bodies()[&1].bounds, None);
    assert_eq!(uploads(&record), [1]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn homes_refusal_answered_after_the_view_was_placed_is_never_said_and_the_view_is_read() {
    // No body can be measured, so the first read, of Home's view, is refused naming
    // `view`. The view placed by the person while that read is out: Home was the app's own
    // ask, so its refusal is said no more than when nobody moved, and the view placed is
    // read next, at once, and kept.
    let f = fixture();
    unmeasured(&f);
    let (mut h, _record) = f.drawn("unmeasured.silico", UNMEASURED);
    step_until(&mut h, "the first read", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.view.reading())
    });
    assert_eq!(view(&h).reads(), 1);
    let placed = silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic { view_height: 80.0 },
        eye: [10.0, -100.0, 10.0],
        target: [10.0; 3],
        up: [0.0, 0.0, 1.0],
    };
    view_mut(&mut h).set_camera(placed.clone());
    step_until(&mut h, "the placed view read", |h| {
        assert_eq!(h.state().tabs()[0].message, None);
        view(h).reads() == 2 && !view(h).reading()
    });
    assert_eq!(view(&h).camera(), &placed);
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_camera_moved_before_the_first_read_answers_isnt_taken_back_by_homes_fit() {
    let f = fixture();
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    step_until(&mut h, "the first read held", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.view.reading())
    });
    let before = view(&h).camera().clone();
    let centre = the_viewport(&h).center();
    wheel(&mut h, centre, eframe::egui::MouseWheelUnit::Point, 200.0);
    let moved = view(&h).camera().clone();
    assert!((ortho_height(&moved) - ortho_height(&before) / 2.0).abs() < 1e-9);
    hold.release();
    meshes_settled(&mut h);
    assert_eq!(view(&h).camera(), &moved);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_perspective_view_reads_each_body_at_its_nearest_points_half_pixel() {
    // From (25, −500, 25) onto (25, 25, 25) at 30°: both cubes' nearest faces are at y = 0,
    // 500 mm deep, so each half pixel is 500 · tan 15° / H. The small cube is meshed at its
    // power of two below; the large one's floor, 0.1732, is coarser than that level, so
    // it's meshed at its floor, serving [0, 0.25).
    let f = fixture();
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(silico_protocol::Camera {
        projection: silico_protocol::Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: [25.0, -500.0, 25.0],
        target: [25.0; 3],
        up: [0.0, 0.0, 1.0],
    });
    meshes_settled(&mut h);
    let height = f64::from(view(&h).frame().unwrap().size_px[1]);
    let half = 500.0 * libm::tan(15f64.to_radians()) / height;
    let level = libm::exp2(libm::floor(libm::log2(half)));
    assert!(level < 0.1732 && 2.0 * level > half);
    let small = &view(&h).bodies()[&1];
    assert_eq!((small.level, small.serves), (level, [level, 2.0 * level]));
    let large = &view(&h).bodies()[&2];
    assert_eq!(large.serves, [0.0, 0.25]);
    assert!((large.deflection - 1000.0 * 3f64.sqrt() * 1e-4).abs() < 1e-12);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_pick_waiting_for_its_answer_asks_for_frames_until_it_comes() {
    // An idle egui draws no frame, so a pick asked asks for one at once until its answer
    // is taken; then the frame asked for is the idle one again.
    let f = fixture();
    let (mut h, record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    for _ in 0..5 {
        step(&mut h);
    }
    let delay = |h: &AppHarness| {
        h.output()
            .viewport_output
            .get(&eframe::egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
    };
    assert!(delay(&h).is_some_and(|delay| delay > Duration::from_millis(500)));
    let rect = silico_app::renderer::PixelRect {
        min: [10, 10],
        size: [3, 3],
    };
    record.lock().unwrap().pick = None;
    assert!(view_mut(&mut h).pick(rect));
    step(&mut h);
    assert_eq!(delay(&h), Some(Duration::ZERO));
    step(&mut h);
    assert_eq!(view(&h).picked().map(|picking| picking.rect), Some(rect));
    step(&mut h);
    assert!(delay(&h).is_some_and(|delay| delay > Duration::from_millis(500)));
    assert_eq!(never_waits::waits(), 0);
}

// ---- The viewport drawn through silico-render on lavapipe ----

/// `test` run where lavapipe is this process's Vulkan driver: in a run of this test alone
/// with lavapipe's variables, if this process hasn't them (a loader's path can't be set
/// once the process runs), as silico-render's own frame tests run. With no lavapipe built
/// in (none in the environment), here, where its check of the adapter fails the test: it
/// never runs on another.
fn on_lavapipe(name: &str, test: impl FnOnce()) {
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
        return;
    }
    test();
}

/// eframe's render state as the app gets it, on lavapipe: egui_kittest's wgpu setup (which
/// takes a CPU adapter first), its device made by egui_wgpu as eframe makes the app's.
fn lavapipe_render_state() -> eframe::egui_wgpu::RenderState {
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

/// The fake kernel's display meshes made the bench's 40 by 20 by 10 box from the origin:
/// six faces in the box's order (Face3 its front, y = 0), two triangles each, and twelve
/// edges, where the fake's own mesh is a triangle a face, all alike.
fn meshes_a_real_box(f: &Fixture) {
    f.kernel.answer_instead(|op| match op {
        Op::Tessellate {
            deflection,
            at_least_the_floor,
            ..
        } => {
            let mesh = silico_render::bench::box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            Some(if *at_least_the_floor {
                silico_protocol::Reply::MeshAt(silico_protocol::MeshAt {
                    mesh,
                    deflection: *deflection,
                })
            } else {
                silico_protocol::Reply::Mesh(mesh)
            })
        }
        _ => None,
    });
}

const A_BOX: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

#[test]
fn the_viewport_draws_through_silico_render_on_lavapipe_and_a_pick_names_the_face_under_it() {
    on_lavapipe(
        "the_viewport_draws_through_silico_render_on_lavapipe_and_a_pick_names_the_face_under_it",
        || {
            let f = fixture();
            meshes_a_real_box(&f);
            let state = lavapipe_render_state();
            // As main.rs does: the renderer on eframe's device, in its callbacks' resources.
            let present = silico_app::renderer::on_eframe(&state).expect("no renderer");
            let app = App::new(Arc::clone(&f.sessions)).presenting(Box::new(present));
            let mut h = {
                let _marked = never_waits::marked();
                Harness::builder()
                    .with_size((1280.0, 800.0))
                    .with_step_dt(1.0 / 60.0)
                    .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                        state.clone(),
                    ))
                    .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
            };
            // Each frame drawn: egui's paint callbacks prepared and painted, as a window's.
            h.set_render_every_step(true);
            let path = f.file("box.silico", A_BOX);
            h.state_mut().open(path.to_str().unwrap(), false);
            meshes_settled(&mut h);
            // From the front, onto the box's centre (20, 10, 5), 20 mm high: its front face,
            // 40 by 10 mm, fills the view's middle; the view's centre is on it.
            view_mut(&mut h).set_camera(silico_protocol::Camera {
                projection: silico_protocol::Projection::Orthographic { view_height: 20.0 },
                eye: [20.0, -100.0, 5.0],
                target: [20.0, 10.0, 5.0],
                up: [0.0, 0.0, 1.0],
            });
            meshes_settled(&mut h);
            for _ in 0..5 {
                step(&mut h);
            }
            let uploads = || {
                state
                    .renderer
                    .read()
                    .callback_resources
                    .get::<silico_app::renderer::Renderer>()
                    .expect("installed")
                    .uploads()
            };
            // Each of the box's six faces uploaded once, and nothing more over twenty frames
            // of an orthographic orbit: bodies move into each view, never re-uploaded.
            let uploaded = uploads();
            assert_eq!(uploaded, 6);
            let front = view(&h).camera().clone();
            let at = the_viewport(&h).center() - eframe::egui::vec2(150.0, 0.0);
            let shift = eframe::egui::Modifiers::SHIFT;
            h.event(eframe::egui::Event::ModifiersChanged(shift));
            h.hover_at(at);
            h.event(eframe::egui::Event::PointerButton {
                pos: at,
                button: eframe::egui::PointerButton::Middle,
                pressed: true,
                modifiers: shift,
            });
            step(&mut h);
            for i in 1..=10u8 {
                h.hover_at(at + eframe::egui::vec2(f32::from(i), 0.0));
                step(&mut h);
            }
            h.event(eframe::egui::Event::PointerButton {
                pos: at + eframe::egui::vec2(10.0, 0.0),
                button: eframe::egui::PointerButton::Middle,
                pressed: false,
                modifiers: shift,
            });
            h.event(eframe::egui::Event::ModifiersChanged(
                eframe::egui::Modifiers::NONE,
            ));
            for _ in 0..10 {
                step(&mut h);
            }
            assert_ne!(view(&h).camera(), &front, "the orbit moved nothing");
            assert_eq!(uploads(), uploaded);
            // Back to the front, and a pick at the view's centre, a frame or two later: the
            // box's front face, Face3, by its name.
            view_mut(&mut h).set_camera(silico_protocol::Camera {
                projection: silico_protocol::Projection::Orthographic { view_height: 20.0 },
                eye: [20.0, -100.0, 5.0],
                target: [20.0, 10.0, 5.0],
                up: [0.0, 0.0, 1.0],
            });
            // Settled, with no read out that could land between the pick and its answer.
            meshes_settled(&mut h);
            for _ in 0..3 {
                step(&mut h);
            }
            let [width, height] = view(&h).frame().unwrap().size_px;
            let centre = silico_app::renderer::PixelRect {
                min: [
                    i32::try_from(width / 2).unwrap() - 2,
                    i32::try_from(height / 2).unwrap() - 2,
                ],
                size: [5, 5],
            };
            assert!(view_mut(&mut h).pick(centre));
            for _ in 0..8 {
                step(&mut h);
                if view(&h).picked().is_some() {
                    break;
                }
            }
            let picked = view(&h)
                .picked()
                .expect("no picking answer within 8 frames");
            assert_eq!(picked.rect, centre);
            let hit = picked.hit.expect("the background at the box's centre");
            assert_eq!(
                (hit.body, hit.hit),
                (1, silico_app::renderer::Element::Face(3))
            );
            let silico_app::renderer::Element::Face(n) = hit.hit else {
                unreachable!()
            };
            let name = view(&h).bodies()[&1].faces[usize::try_from(n - 1).unwrap()]
                .name
                .to_string();
            assert_eq!(name, "k7f2qx3m.box.Face3");
            // On the front face, y = 0, at about the view's centre's x and z.
            let point = hit.point.expect("no point");
            assert!(point[1].abs() < 1e-3, "{point:?}");
            assert!(
                (point[0] - 20.0).abs() < 0.1 && (point[2] - 5.0).abs() < 0.1,
                "{point:?}"
            );
            // Composited into egui's own image: 20 mm over the pane's height, the face
            // z = 0..10 about its centre's z = 5, so 2.5 mm above the pane's centre is on the
            // face and 7.5 mm above it is past its top edge, the renderer's background; and
            // the centre isn't the pane's own colour, which shows where nothing composites.
            let image = h.render().unwrap();
            let rect = the_viewport(&h);
            let ppp = h.ctx.pixels_per_point();
            let mm = rect.height() / 20.0;
            let pixel = |x: f32, y: f32| {
                let at = |v: f32| v * ppp;
                let (x, y) = (at(x).round() as u32, at(y).round() as u32);
                image.get_pixel(x, y).0
            };
            let differs = |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 8);
            let centre = rect.center();
            let on_face = pixel(centre.x, centre.y - 2.5 * mm);
            let past_it = pixel(centre.x, centre.y - 7.5 * mm);
            assert!(differs(on_face, past_it), "{on_face:?} against {past_it:?}");
            let pane = h.ctx.global_style().visuals.extreme_bg_color.to_array();
            let middle = pixel(centre.x, centre.y);
            assert!(differs(middle, pane), "{middle:?}: the pane's own colour");
            // The pane's renderer trait reaches silico-render's own: a frame drawing body 1
            // is prepared while body 1 is held, and once the trait has forgotten it, refused,
            // as silico-render refuses a frame drawing a body it doesn't hold.
            {
                use silico_app::renderer::ViewRenderer;
                let mut resources = state.renderer.write();
                let renderer = resources
                    .callback_resources
                    .get_mut::<silico_app::renderer::Renderer>()
                    .expect("installed");
                let data = silico_app::renderer::FrameData {
                    serial: 1 << 20,
                    camera: view(&h).camera().clone(),
                    size_px: [64, 64],
                    pixels_per_point: 1.0,
                    bodies: vec![1],
                    highlights: Vec::new(),
                    picking: None,
                };
                assert!(ViewRenderer::prepare(renderer, &data.frame()).is_ok());
                ViewRenderer::forget(renderer, 1);
                let refused = ViewRenderer::prepare(renderer, &data.frame())
                    .expect_err("a frame of a body forgotten");
                assert_eq!(refused.to_string(), "bodies[0], 1, isn't uploaded");
            }
            assert_eq!(never_waits::waits(), 0);
        },
    );
}

#[test]
fn a_viewport_with_no_renderer_says_why_in_its_pane() {
    let f = fixture();
    let app = App::new(Arc::clone(&f.sessions)).not_drawing("no adapter was found".to_owned());
    let mut h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    };
    step(&mut h);
    // With no document, the start page shows, and no viewport.
    assert!(h.query_by_role_and_label(Role::Pane, "Viewport").is_none());
    assert!(h.query_by_role_and_label(Role::Pane, "Start").is_some());
    // With a document open, its viewport says why it draws nothing.
    let path = f.file("example.silico", EXAMPLE);
    h.state_mut().open(path.to_str().unwrap(), false);
    step_until(&mut h, "the tab", |h| !h.state().tabs().is_empty());
    assert!(shows(
        &h,
        "Viewport",
        "The viewport can't draw: no adapter was found"
    ));
    assert_eq!(never_waits::waits(), 0);
}

// ---- What draws over the viewport, and the left button handed on ----

/// The colour the overlay test marks the bodies' place with (as the paint callback would
/// stand there), and the overlay's own.
const BODIES_MARK: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(1, 2, 3);
const OVERLAY_MARK: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(4, 5, 6);

/// What the tabs' overlays were shown: the serial of the last frame presented, until an
/// overlay takes it; and each overlay's call, with the serial it took (none for a call with
/// no frame presented since the last, as a tab no longer shown is given its cancel), its
/// ViewFrame, and egui's input time.
#[derive(Default)]
struct Overlaid {
    serial: Option<u64>,
    frames: Vec<(Option<u64>, silico_app::viewport::ViewFrame, f64)>,
}

type Overlays = Arc<Mutex<Overlaid>>;

/// A present that marks where the bodies' paint callback stands, and keeps each frame's
/// serial for the overlay that follows it.
struct Marking(Overlays);

impl silico_app::renderer::Present for Marking {
    fn present(
        &mut self,
        painter: &eframe::egui::Painter,
        rect: eframe::egui::Rect,
        _: Vec<silico_app::renderer::Change>,
        frame: Option<silico_app::renderer::FrameData>,
    ) {
        if let Some(frame) = frame {
            painter.rect_filled(
                eframe::egui::Rect::from_min_size(rect.min, eframe::egui::vec2(3.0, 3.0)),
                0.0,
                BODIES_MARK,
            );
            self.0.lock().unwrap().serial = Some(frame.serial);
        }
    }
    fn picked(&mut self) -> Option<silico_app::renderer::Picking> {
        None
    }
    fn refused(&mut self) -> std::collections::BTreeSet<u32> {
        std::collections::BTreeSet::new()
    }
}

/// A tab's watch that keeps each call in `seen`, and marks the viewport's centre.
fn watching(seen: &Overlays) -> silico_app::tab::Watch {
    let kept = Arc::clone(seen);
    Box::new(move |painter, frame| {
        let time = painter.ctx().input(|input| input.time);
        let mut seen = kept.lock().unwrap();
        let serial = seen.serial.take();
        seen.frames.push((serial, frame.clone(), time));
        painter.circle_filled(frame.rect.center(), 4.0, OVERLAY_MARK);
    })
}

impl Fixture {
    /// The app with `text` open, at two physical pixels a point, its viewport marked and its
    /// tab watched, every overlay's call kept.
    fn overlaid(&self, text: &str) -> (AppHarness, Overlays) {
        let path = self.file("example.silico", text);
        let seen = Overlays::default();
        let app =
            App::new(Arc::clone(&self.sessions)).presenting(Box::new(Marking(Arc::clone(&seen))));
        let mut h = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size((1280.0, 800.0))
                .with_pixels_per_point(2.0)
                .with_step_dt(1.0 / 60.0)
                .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
        };
        h.state_mut().open(path.to_str().unwrap(), false);
        meshes_settled(&mut h);
        h.state_mut().tabs_mut()[0].watch = Some(watching(&seen));
        (h, seen)
    }
}

/// Every shape the last frame painted, in paint order, nested ones flattened.
fn painted(h: &AppHarness) -> Vec<eframe::egui::Shape> {
    fn flat(shape: &eframe::egui::Shape, into: &mut Vec<eframe::egui::Shape>) {
        match shape {
            eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| flat(s, into)),
            other => into.push(other.clone()),
        }
    }
    let mut shapes = Vec::new();
    for clipped in &h.output().shapes {
        flat(&clipped.shape, &mut shapes);
    }
    shapes
}

#[test]
fn the_overlay_sees_the_frame_and_paints_over_the_bodies_and_under_the_view_cube() {
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    step(&mut h);
    // What it was shown is the frame as the pane keeps it, of the tab's session, after the
    // bodies' frame was presented.
    {
        let seen = seen.lock().unwrap();
        let (serial, frame, _) = seen.frames.last().unwrap();
        assert!(serial.is_some(), "the bodies' frame first");
        assert_eq!(Some(frame), view(&h).frame());
        assert_eq!(frame.session, h.state().tabs()[0].id);
    }
    let shapes = painted(&h);
    let place = |what: &str, found: &dyn Fn(&eframe::egui::Shape) -> bool| {
        shapes
            .iter()
            .position(found)
            .unwrap_or_else(|| panic!("no {what} painted"))
    };
    let bodies = place(
        "bodies' mark",
        &|shape| matches!(shape, eframe::egui::Shape::Rect(r) if r.fill == BODIES_MARK),
    );
    let overlay = place(
        "overlay",
        &|shape| matches!(shape, eframe::egui::Shape::Circle(c) if c.fill == OVERLAY_MARK),
    );
    // The ViewCube's Front face's label, and the navigation bar's Fit.
    let text = |word: &'static str| move |shape: &eframe::egui::Shape| matches!(shape, eframe::egui::Shape::Text(t) if t.galley.text() == word);
    let cube = place("ViewCube's Front", &text("Front"));
    let bar = place("bar's Fit", &text("Fit"));
    assert!(bodies < overlay, "{bodies} {overlay}");
    assert!(overlay < cube && overlay < bar, "{overlay} {cube} {bar}");
    assert_eq!(never_waits::waits(), 0);
}

/// A left-button event as a tab's overlay was given it: the serial its call took, the
/// frame's session, the event, and egui's time at the call.
#[derive(Debug, Clone, Copy)]
struct LeftSeen {
    serial: Option<u64>,
    session: silico_core::command::SessionId,
    event: silico_app::viewport::LeftEvent,
    time: f64,
}

/// Every left-button event the overlays were given, in order, and the calls cleared.
fn left_taken(seen: &Overlays) -> Vec<LeftSeen> {
    let mut seen = seen.lock().unwrap();
    let left = seen
        .frames
        .iter()
        .flat_map(|(serial, frame, time)| {
            frame.left.iter().map(move |event| LeftSeen {
                serial: *serial,
                session: frame.session,
                event: *event,
                time: *time,
            })
        })
        .collect();
    seen.frames.clear();
    left
}

fn kinds(events: &[LeftSeen]) -> Vec<silico_app::viewport::LeftKind> {
    events.iter().map(|seen| seen.event.kind).collect()
}

fn button_event(
    h: &AppHarness,
    at: eframe::egui::Pos2,
    button: eframe::egui::PointerButton,
    pressed: bool,
    modifiers: eframe::egui::Modifiers,
) {
    h.event(eframe::egui::Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers,
    });
}

/// A press at `from`, a move to `to`, and a release there, `held` the whole time, each
/// event a frame of its own; then frames enough for them all.
fn left_drag(
    h: &mut AppHarness,
    from: eframe::egui::Pos2,
    to: eframe::egui::Pos2,
    held: eframe::egui::Modifiers,
) {
    let left = eframe::egui::PointerButton::Primary;
    h.event(eframe::egui::Event::ModifiersChanged(held));
    h.hover_at(from);
    button_event(h, from, left, true, held);
    h.hover_at(to);
    button_event(h, to, left, false, held);
    h.event(eframe::egui::Event::ModifiersChanged(
        eframe::egui::Modifiers::NONE,
    ));
    for _ in 0..3 {
        step(h);
    }
}

/// Where `at`, in points, is in the viewport `rect` at two physical pixels a point.
fn at_px(rect: eframe::egui::Rect, at: eframe::egui::Pos2) -> [f32; 2] {
    let d = (at - rect.min) * 2.0;
    [d.x, d.y]
}

#[test]
fn the_left_buttons_events_on_the_canvas_are_handed_on_with_their_frame_and_time() {
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2, vec2};
    use silico_app::viewport::LeftKind::{DoubleClick, Move, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    assert_eq!(h.ctx.pixels_per_point(), 2.0);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let to = at + vec2(10.0, 5.0);
    let left = PointerButton::Primary;
    let none = Modifiers::NONE;
    left_taken(&seen);
    // Shift without Ctrl: a press, a move, and a release, each a frame of its own, at
    // physical pixels from the viewport's top left, each of the frame it came in, at
    // egui's time then.
    left_drag(&mut h, at, to, Modifiers::SHIFT);
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Move, Release]);
    let places: Vec<[f32; 2]> = events.iter().map(|seen| seen.event.at_px).collect();
    assert_eq!(places, [at_px(rect, at), at_px(rect, to), at_px(rect, to)]);
    for seen in &events {
        assert!(seen.event.shift && !seen.event.ctrl, "{seen:?}");
        assert_eq!(Some(seen.event.frame), seen.serial, "{seen:?}");
        assert_eq!(seen.event.time, seen.time, "{seen:?}");
    }
    for pair in events.windows(2) {
        assert!(pair[0].event.frame < pair[1].event.frame, "{pair:?}");
        assert!(pair[0].event.time < pair[1].event.time, "{pair:?}");
    }
    // Ctrl (Cmd on macOS) without Shift.
    left_drag(&mut h, at, to, Modifiers::COMMAND);
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Move, Release]);
    for seen in &events {
        assert!(seen.event.ctrl && !seen.event.shift, "{seen:?}");
    }
    // Two clicks a frame apart, well after the drags (egui's delay is 0.3 s): each press
    // and release handed on, then egui's double-click, in the frame of the second release,
    // where it was.
    for _ in 0..30 {
        step(&mut h);
    }
    left_taken(&seen);
    let there = at + vec2(30.0, -20.0);
    h.hover_at(there);
    for _ in 0..2 {
        button_event(&h, there, left, true, none);
        button_event(&h, there, left, false, none);
    }
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    assert_eq!(
        kinds(&events),
        [Press, Release, Press, Release, DoubleClick]
    );
    assert_eq!(events[4].event.at_px, at_px(rect, there));
    assert_eq!(events[4].event.frame, events[3].event.frame);
    // A drag let go outside the viewport: its moves and its release past its left edge
    // (negative) and past its bottom (beyond its height), unclamped.
    let size_px = view(&h).frame().unwrap().size_px;
    let past_left = Pos2::new(rect.left() - 40.0, at.y);
    let below = Pos2::new(at.x, rect.bottom() + 30.0);
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    h.hover_at(past_left);
    h.hover_at(below);
    button_event(&h, below, left, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Move, Move, Release]);
    assert_eq!(events[1].event.at_px, at_px(rect, past_left));
    assert!(events[1].event.at_px[0] < 0.0, "{:?}", events[1]);
    assert_eq!(events[3].event.at_px, at_px(rect, below));
    assert!(
        events[3].event.at_px[1] > size_px[1] as f32,
        "{:?}",
        events[3]
    );
    // Several events in one frame: handed on in their order, each of that frame and time.
    for _ in 0..30 {
        step(&mut h);
    }
    left_taken(&seen);
    let (a, b, c) = (at, at + vec2(5.0, 0.0), at + vec2(5.0, 7.0));
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: left,
        pressed,
        modifiers: none,
    };
    h.input_mut().events.extend([
        Event::PointerMoved(a),
        button(a, true),
        Event::PointerMoved(b),
        Event::PointerMoved(c),
        button(c, false),
    ]);
    step(&mut h);
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Move, Move, Release]);
    let places: Vec<[f32; 2]> = events.iter().map(|seen| seen.event.at_px).collect();
    let expected = [a, b, c, c].map(|p| at_px(rect, p));
    assert_eq!(places, expected);
    // Handed on once: the frame the view keeps holds none of them.
    assert!(view(&h).frame().unwrap().left.is_empty());
    for seen in &events {
        assert_eq!(Some(seen.event.frame), seen.serial, "{seen:?}");
        assert_eq!(seen.event.time, seen.time, "{seen:?}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_press_with_no_release_before_the_next_is_cancelled_first() {
    use eframe::egui::{Event, Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Cancel, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let (a, b) = (rect.center(), rect.center() + vec2(40.0, 0.0));
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    left_taken(&seen);
    h.input_mut()
        .events
        .extend([Event::PointerMoved(a), button(a, true), button(b, true)]);
    step(&mut h);
    h.event(button(b, false));
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Cancel, Press, Release]);
    // The first press's cancel is where it was; the second's release, where it was let go.
    assert_eq!(events[1].event.at_px, at_px(rect, a));
    assert_eq!(events[3].event.at_px, at_px(rect, b));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_left_drag_cut_short_by_the_focus_going_ends_in_a_cancel() {
    use eframe::egui::{Event, Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Cancel, Move, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let to = at + vec2(10.0, 5.0);
    let left = PointerButton::Primary;
    let none = Modifiers::NONE;
    left_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    h.hover_at(to);
    h.event(Event::WindowFocused(false));
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Move, Cancel]);
    // Where the stream was last.
    assert_eq!(events[2].event.at_px, at_px(rect, to));
    // A move and a release after it, with no press: nothing.
    h.hover_at(at);
    h.hover_at(to);
    button_event(&h, to, left, false, none);
    h.hover_at(at);
    for _ in 0..3 {
        step(&mut h);
    }
    let taken = left_taken(&seen);
    assert!(taken.is_empty(), "{taken:?}");
    // The next press begins again: what the above wasn't missing for want of events.
    left_drag(&mut h, at, to, none);
    assert_eq!(kinds(&left_taken(&seen)), [Press, Move, Release]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_left_drag_goes_on_when_the_pointer_leaves_the_window_until_its_release() {
    use eframe::egui::{Event, Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Move, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let to = at + vec2(10.0, 5.0);
    let left = PointerButton::Primary;
    left_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, left, true, Modifiers::NONE);
    h.hover_at(to);
    h.event(Event::PointerGone);
    for _ in 0..3 {
        step(&mut h);
    }
    assert_eq!(kinds(&left_taken(&seen)), [Press, Move]);
    // The button comes up where the pointer comes back: the drag's release.
    button_event(&h, to, left, false, Modifiers::NONE);
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Release]);
    assert_eq!(events[0].event.at_px, at_px(rect, to));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_left_drag_a_navigation_tool_takes_ends_in_a_cancel() {
    use eframe::egui::{Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Cancel, Press, Release};
    use silico_app::viewport::Navigation;
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let left = PointerButton::Primary;
    let none = Modifiers::NONE;
    left_taken(&seen);
    // A tool chosen mid-drag (as a client might): the drag is the tool's from then on.
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    step(&mut h);
    view_mut(&mut h).tool = Some(Navigation::Orbit);
    step(&mut h);
    button_event(&h, at, left, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Cancel], "{events:?}");
    assert_eq!(events[1].event.at_px, at_px(rect, at));
    view_mut(&mut h).tool = None;
    for _ in 0..30 {
        step(&mut h);
    }
    // And with no tool, a press and release on the canvas are handed on.
    for _ in 0..30 {
        step(&mut h);
    }
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    button_event(&h, at, left, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    assert_eq!(kinds(&left_taken(&seen)), [Press, Release]);
    assert_eq!(never_waits::waits(), 0);
}

/// `h` with a second document open beside the first, the first shown again, both tabs
/// watched: the first's session and the second's.
fn two_tabs_first_shown(
    f: &Fixture,
    h: &mut AppHarness,
    seen: &Overlays,
) -> (
    silico_core::command::SessionId,
    silico_core::command::SessionId,
) {
    let other = f.file("other.silico", EXAMPLE);
    h.state_mut().open(other.to_str().unwrap(), false);
    step_until(h, "two tabs", |h| h.state().tabs().len() == 2);
    h.get_by_role_and_label(Role::Button, "example.silico")
        .click();
    let first = h.state().tabs()[0].id;
    step_until(h, "the first shown", |h| left_shown(h) == Some(first));
    h.state_mut().tabs_mut()[1].watch = Some(watching(seen));
    for _ in 0..3 {
        step(h);
    }
    left_taken(seen);
    (first, h.state().tabs()[1].id)
}

/// The session of the tab shown, as the tab bar marks it.
fn left_shown(h: &AppHarness) -> Option<silico_core::command::SessionId> {
    h.state()
        .tabs()
        .iter()
        .find(|tab| {
            h.get_by_role_and_label(Role::Button, tab.name())
                .accesskit_node()
                .toggled()
                == Some(eframe::egui::accesskit::Toggled::True)
        })
        .map(|tab| tab.id)
}

#[test]
fn a_left_drag_in_a_tab_that_closes_ends_with_it_and_the_next_press_begins_afresh() {
    use eframe::egui::{Key, Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Move, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let (first, second) = two_tabs_first_shown(&f, &mut h, &seen);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let to = at + vec2(10.0, 5.0);
    let left = PointerButton::Primary;
    let none = Modifiers::NONE;
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    h.hover_at(to);
    step(&mut h);
    // Ctrl+W mid-drag: the first tab closes, its drag with it.
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    step_until(&mut h, "one tab", |h| h.state().tabs().len() == 1);
    assert_eq!(h.state().tabs()[0].id, second);
    // A press in the tab shown, the first's release never having come: a stream of its
    // own, with no cancel of the first's before it.
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    button_event(&h, at, left, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    let firsts: Vec<_> = events
        .iter()
        .filter(|e| e.session == first)
        .copied()
        .collect();
    let seconds: Vec<_> = events
        .iter()
        .filter(|e| e.session == second)
        .copied()
        .collect();
    // The first's closed with it: the press and move it was given, and nothing after.
    assert_eq!(kinds(&firsts), [Press, Move], "{events:?}");
    assert_eq!(kinds(&seconds), [Press, Release], "{events:?}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_release_in_the_frame_the_tab_changes_releases_the_drag_in_the_tab_it_was_pressed_in() {
    use eframe::egui::{Event, Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Move, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let first = h.state().tabs()[0].id;
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let to = at + vec2(10.0, 5.0);
    let left = PointerButton::Primary;
    let none = Modifiers::NONE;
    left_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    h.hover_at(to);
    step(&mut h);
    // A second document's open, answered with no frame drawn (the core's waker says so):
    // the next frame both takes it, showing its tab, and has the drag's release.
    let woken = h.state().wakes();
    let other = f.file("other.silico", EXAMPLE);
    h.state_mut().open(other.to_str().unwrap(), false);
    let deadline = Instant::now() + WAIT;
    while h.state().wakes() == woken {
        assert!(Instant::now() < deadline, "the open never answered");
        std::thread::sleep(Duration::from_millis(2));
    }
    h.input_mut().events.push(Event::PointerButton {
        pos: to,
        button: left,
        pressed: false,
        modifiers: none,
    });
    step(&mut h);
    assert_eq!(
        h.state().tabs().len(),
        2,
        "the open wasn't taken in that frame"
    );
    let second = h.state().tabs()[1].id;
    assert_eq!(left_shown(&h), Some(second));
    h.state_mut().tabs_mut()[1].watch = Some(watching(&seen));
    h.hover_at(at);
    for _ in 0..3 {
        step(&mut h);
    }
    let events = left_taken(&seen);
    assert_eq!(kinds(&events), [Press, Move, Release], "{events:?}");
    assert!(events.iter().all(|e| e.session == first), "{events:?}");
    assert_eq!(events[2].event.at_px, at_px(rect, to));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_left_drag_in_a_tab_no_longer_shown_is_cancelled_there_and_the_tab_shown_given_none() {
    use eframe::egui::{Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Cancel, Move, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let first = h.state().tabs()[0].id;
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let to = at + vec2(10.0, 5.0);
    let left = PointerButton::Primary;
    let none = Modifiers::NONE;
    left_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, left, true, none);
    h.hover_at(to);
    step(&mut h);
    // An open answering mid-drag: its tab is shown over the first.
    let other = f.file("other.silico", EXAMPLE);
    h.state_mut().open(other.to_str().unwrap(), false);
    step_until(&mut h, "the second tab", |h| h.state().tabs().len() == 2);
    let second = h.state().tabs()[1].id;
    assert!(h.state().tabs()[1].view.frame().is_some(), "not shown");
    h.state_mut().tabs_mut()[1].watch = Some(watching(&seen));
    // The release, and moves after it.
    h.hover_at(at);
    button_event(&h, at, left, false, none);
    h.hover_at(to);
    for _ in 0..3 {
        step(&mut h);
    }
    // The second tab was shown and watched all the while.
    assert!(
        seen.lock()
            .unwrap()
            .frames
            .iter()
            .any(|(_, frame, _)| frame.session == second)
    );
    let events = left_taken(&seen);
    let of = |session| {
        events
            .iter()
            .filter(|seen| seen.session == session)
            .copied()
            .collect::<Vec<_>>()
    };
    let firsts = of(first);
    assert_eq!(kinds(&firsts), [Press, Move, Cancel], "{events:?}");
    assert_eq!(firsts[2].event.at_px, at_px(rect, to));
    // Given before the frame was presented, to a tab not drawn in it.
    assert_eq!(firsts[2].serial, None);
    assert!(of(second).is_empty(), "{events:?}");
    // A press of its own is handed on to it.
    left_drag(&mut h, at, to, none);
    let events = left_taken(&seen);
    assert!(events.iter().all(|seen| seen.session == second));
    assert_eq!(kinds(&events), [Press, Move, Release]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn no_left_event_is_handed_on_while_a_tool_navigates_nor_for_the_view_cube_middle_or_wheel() {
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let at = rect.center() - eframe::egui::vec2(100.0, 0.0);
    let none = eframe::egui::Modifiers::NONE;
    let drag = |h: &mut AppHarness, button| {
        h.hover_at(at);
        button_event(h, at, button, true, none);
        h.hover_at(at + eframe::egui::vec2(20.0, 0.0));
        button_event(h, at + eframe::egui::vec2(20.0, 0.0), button, false, none);
        for _ in 0..4 {
            step(h);
        }
    };
    // The Orbit tool chosen: a left drag orbits, and nothing is handed on.
    h.get_by_role_and_label(Role::Toolbar, "Navigation bar")
        .get_by_label("Orbit")
        .click();
    step(&mut h);
    let camera = view(&h).camera().clone();
    left_taken(&seen);
    drag(&mut h, eframe::egui::PointerButton::Primary);
    assert_ne!(view(&h).camera(), &camera, "the tool didn't orbit");
    assert!(left_taken(&seen).is_empty());
    h.get_by_role_and_label(Role::Toolbar, "Navigation bar")
        .get_by_label("Orbit")
        .click();
    step(&mut h);
    // A ViewCube click, the middle button, and the wheel: none either.
    left_taken(&seen);
    h.get_by_role_and_label(Role::Group, "ViewCube")
        .get_by_label("Front")
        .click();
    for _ in 0..3 {
        step(&mut h);
    }
    drag(&mut h, eframe::egui::PointerButton::Middle);
    h.hover_at(at);
    h.event(eframe::egui::Event::MouseWheel {
        unit: eframe::egui::MouseWheelUnit::Point,
        delta: eframe::egui::vec2(0.0, 50.0),
        phase: eframe::egui::TouchPhase::Move,
        modifiers: none,
    });
    for _ in 0..3 {
        step(&mut h);
    }
    let taken = left_taken(&seen);
    assert!(taken.is_empty(), "{taken:?}");
    // And with no tool, the same left drag is handed on: what the test above wasn't
    // missing for want of events.
    drag(&mut h, eframe::egui::PointerButton::Primary);
    assert_eq!(left_taken(&seen).len(), 3);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_left_press_in_the_navigation_bars_padding_or_during_a_middle_pan_is_not_handed_on() {
    use eframe::egui::{Modifiers, PointerButton, Pos2, vec2};
    use silico_app::viewport::LeftKind::{Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let left = PointerButton::Primary;
    let none = Modifiers::NONE;
    // Left of Orbit, inside the bar's frame (its padding is 6 points each side).
    let orbit = h
        .get_by_role_and_label(Role::Toolbar, "Navigation bar")
        .get_by_label("Orbit")
        .rect();
    let padding = Pos2::new(orbit.left() - 3.0, orbit.center().y);
    assert!(rect.contains(padding));
    left_taken(&seen);
    h.hover_at(padding);
    button_event(&h, padding, left, true, none);
    button_event(&h, padding, left, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let taken = left_taken(&seen);
    assert!(taken.is_empty(), "{taken:?}");
    assert_eq!(view(&h).tool, None);
    // A left click while the middle button pans: the pan's, not handed on.
    let at = rect.center() - vec2(100.0, 0.0);
    let on = at + vec2(5.0, 0.0);
    h.hover_at(at);
    button_event(&h, at, PointerButton::Middle, true, none);
    h.hover_at(on);
    button_event(&h, on, left, true, none);
    button_event(&h, on, left, false, none);
    button_event(&h, on, PointerButton::Middle, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let taken = left_taken(&seen);
    assert!(taken.is_empty(), "{taken:?}");
    // The same click with neither, well after: handed on, so the two above weren't
    // missing for want of events.
    for _ in 0..30 {
        step(&mut h);
    }
    h.hover_at(on);
    button_event(&h, on, left, true, none);
    button_event(&h, on, left, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    assert_eq!(kinds(&left_taken(&seen)), [Press, Release]);
    assert_eq!(never_waits::waits(), 0);
}

// ---- Native file dialogs: Browse… beside each path field ----
//
// The platform's own dialog (rfd's, `browse::Native`) needs a window server and a person, so
// it can't run headless: these drive a stand-in through the same `Browse` trait, which
// records what each dialog was asked and answers when the test says, from a thread of its
// own, as the platform's completion does.

use silico_app::browse::{self as browse_mod, Ask, Browse, Pick};

/// The stand-in's state: what each dialog was asked, the answer the test gave (`None` until
/// it does), the waker of the last poll, and how many polls there were.
#[derive(Default)]
struct Dialogs {
    asked: Vec<Ask>,
    answer: Option<Option<PathBuf>>,
    waker: Option<std::task::Waker>,
    polls: u64,
}

#[derive(Clone, Default)]
struct FakeBrowser(Arc<Mutex<Dialogs>>);

impl Browse for FakeBrowser {
    fn browse(&mut self, ask: &Ask) -> browse_mod::Answer {
        let mut dialogs = self.0.lock().unwrap();
        dialogs.asked.push(ask.clone());
        dialogs.answer = None;
        Box::pin(FakeAnswer(Arc::clone(&self.0)))
    }
}

struct FakeAnswer(Arc<Mutex<Dialogs>>);

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
    fn answer(&self, answer: Option<&Path>) {
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

    fn asked(&self) -> Vec<Ask> {
        self.0.lock().unwrap().asked.clone()
    }

    fn polls(&self) -> u64 {
        self.0.lock().unwrap().polls
    }
}

impl Fixture {
    /// The app on these sessions with `browser` opening its file dialogs, nothing open.
    fn app_browsing(&self, browser: &FakeBrowser) -> AppHarness {
        let app = App::new(Arc::clone(&self.sessions)).browsing(Box::new(browser.clone()));
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    }

    /// [`opened`](Self::opened), with `browser` opening its file dialogs.
    fn opened_browsing(
        &self,
        browser: &FakeBrowser,
        name: &str,
        text: &str,
    ) -> (AppHarness, PathBuf) {
        let path = self.file(name, text);
        let mut h = self.app_browsing(browser);
        h.state_mut().open(path.to_str().unwrap(), false);
        step_until(&mut h, "the timeline read", |h| {
            h.state()
                .tabs()
                .first()
                .is_some_and(|tab| tab.timeline.is_some())
        });
        (h, path)
    }
}

/// File > `item`, its dialog (headed `heading`) open.
fn file_menu(h: &mut AppHarness, item: &str, heading: &str) {
    h.get_by_label("File").click();
    step(h);
    step(h);
    button_off_start(h, item).click();
    step_until(h, heading, |h| h.query_by_label_contains(heading).is_some());
}

/// The dialog's Browse… button.
fn browse_button(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::Button, "Browse…")
}

/// Browse… clicked, and the native dialog open on its question.
fn browse(h: &mut AppHarness, browser: &FakeBrowser) {
    let asked = browser.asked().len();
    browse_button(h).click_accesskit();
    step_until(h, "the native dialog asked", |_| {
        browser.asked().len() > asked
    });
}

/// Requests for a frame at once, counted as egui is asked for them.
fn count_repaints(h: &AppHarness) -> Arc<std::sync::atomic::AtomicU64> {
    let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counted = Arc::clone(&count);
    h.ctx.set_request_repaint_callback(move |info| {
        if info.delay == Duration::ZERO {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    count
}

/// The value of the dialog's Path field.
fn path_value(h: &AppHarness) -> String {
    h.get_by_role_and_label(Role::TextInput, "Path")
        .value()
        .unwrap()
}

#[test]
fn with_no_native_dialogs_no_browse_button_is_shown() {
    let f = fixture();
    let mut h = f.app();
    file_menu(&mut h, "Open…", "Open document");
    assert!(h.query_by_role_and_label(Role::Button, "Browse…").is_none());
}

#[test]
fn opens_browse_asks_for_a_silico_file_frames_climb_while_it_is_open_and_its_pick_opens() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    file_menu(&mut h, "Open…", "Open document");
    // A path typed first, which the pick's open takes as a typed one's would.
    h.get_by_role(Role::TextInput).focus();
    step(&mut h);
    h.get_by_role(Role::TextInput).type_text("typed.silico");
    step(&mut h);
    browse(&mut h, &browser);
    // Polled in the frame it opened (rfd's portal starts its dialog on the first poll).
    assert_eq!(browser.polls(), 1);
    // Nothing open, no folder picked before: the platform's own start.
    assert_eq!(
        browser.asked(),
        [Ask {
            pick: Pick::Open,
            title: "Open document".to_owned(),
            filters: vec![browse_mod::DOCUMENTS],
            folder: None,
            file_name: None,
        }]
    );
    assert_eq!(browse_mod::DOCUMENTS.extensions, ["silico"]);
    // While it's open: frames climb, each polling it, never waiting; Browse… is off, so no
    // second dialog opens over it.
    let (frames, polls) = (h.state().frames(), browser.polls());
    for _ in 0..50 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), frames + 50);
    assert!(browser.polls() >= polls + 50, "{}", browser.polls());
    assert!(browse_button(&h).accesskit_node().is_disabled());
    browse_button(&h).click_accesskit();
    step(&mut h);
    assert_eq!(browser.asked().len(), 1);
    assert!(h.state().tabs().is_empty());
    // Its answer, from another thread, asks egui for a frame at once: none asked for until
    // then (the click's own frames settled), so the one asked for is the answer's.
    for _ in 0..5 {
        step(&mut h);
    }
    let path = f.file("picked.silico", EXAMPLE);
    let repaints = count_repaints(&h);
    browser.answer(Some(&path));
    assert!(repaints.load(Ordering::SeqCst) >= 1);
    step_until(&mut h, "its tab", |h| anywhere(h, "picked.silico"));
    assert_eq!(h.state().tabs()[0].path, path.to_str().unwrap());
    assert!(!anywhere(&h, "Open document"));
    // Open again: its field emptied, and the shown document's folder is where it starts.
    file_menu(&mut h, "Open…", "Open document");
    assert_eq!(path_value(&h), "");
    browse(&mut h, &browser);
    assert_eq!(browser.asked()[1].folder.as_deref(), Some(f.dir.path()));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancelled_browse_changes_nothing() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, path) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    // Open, a path typed first: cancelled, the field holds what was typed and nothing opens.
    file_menu(&mut h, "Open…", "Open document");
    h.get_by_role(Role::TextInput).focus();
    step(&mut h);
    h.get_by_role(Role::TextInput).type_text("typed.silico");
    step(&mut h);
    browse(&mut h, &browser);
    browser.answer(None);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(path_value(&h), "typed.silico");
    assert!(anywhere(&h, "Open document"));
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(h.state().message(), None);
    // Browse… is on again.
    assert!(!browse_button(&h).accesskit_node().is_disabled());
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| !anywhere(h, "Open document"));
    // Save As: cancelled, its field stays empty and nothing is saved.
    file_menu(&mut h, "Save As…", "Save example.silico as");
    browse(&mut h, &browser);
    browser.answer(None);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(path_value(&h), "");
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert_eq!(h.state().tabs()[0].path, path.to_str().unwrap());
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| {
        !anywhere(h, "Save example.silico as")
    });
    // Export: cancelled, its path is the one it had.
    open_export(&mut h);
    let before = path_value(&h);
    browse(&mut h, &browser);
    browser.answer(None);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(path_value(&h), before);
    assert!(h.state().tabs()[0].exports.is_empty());
    // Each started in the document's folder, the cancels before picking none
    // (`a_cancel_picks_no_folder` shows that with no document open).
    assert!(
        browser
            .asked()
            .iter()
            .all(|ask| ask.folder.as_deref() == Some(f.dir.path()))
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- The right button's clicks handed on ----

/// A right click as a tab's overlay was given it: the serial its call took, the frame's
/// session, the click, and egui's time at the call.
#[derive(Debug, Clone, Copy)]
struct SecondarySeen {
    serial: Option<u64>,
    session: silico_core::command::SessionId,
    click: silico_app::viewport::SecondaryClick,
    time: f64,
}

/// Every right click the overlays were given, in order, the calls kept (for
/// [`left_taken`], which clears them).
fn secondary_seen(seen: &Overlays) -> Vec<SecondarySeen> {
    let seen = seen.lock().unwrap();
    seen.frames
        .iter()
        .flat_map(|(serial, frame, time)| {
            frame.secondary.iter().map(move |click| SecondarySeen {
                serial: *serial,
                session: frame.session,
                click: *click,
                time: *time,
            })
        })
        .collect()
}

/// Every right click the overlays were given, in order, and the calls cleared.
fn secondary_taken(seen: &Overlays) -> Vec<SecondarySeen> {
    let clicks = secondary_seen(seen);
    seen.lock().unwrap().frames.clear();
    clicks
}

/// A right press at `from` and its release at `to`, the pointer moved there between,
/// `held` the whole time, each event a frame of its own; then frames enough for them all.
fn right_click(
    h: &mut AppHarness,
    from: eframe::egui::Pos2,
    to: eframe::egui::Pos2,
    held: eframe::egui::Modifiers,
) {
    let right = eframe::egui::PointerButton::Secondary;
    h.event(eframe::egui::Event::ModifiersChanged(held));
    h.hover_at(from);
    button_event(h, from, right, true, held);
    h.hover_at(to);
    button_event(h, to, right, false, held);
    h.event(eframe::egui::Event::ModifiersChanged(
        eframe::egui::Modifiers::NONE,
    ));
    for _ in 0..3 {
        step(h);
    }
}

#[test]
fn a_right_click_on_the_canvas_is_handed_on_once_where_it_was_let_go_with_its_modifiers() {
    use eframe::egui::{Modifiers, vec2};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    assert_eq!(h.ctx.pixels_per_point(), 2.0);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    secondary_taken(&seen);
    // Shift without Ctrl: one click, at physical pixels from the viewport's top left, of
    // the frame it came in, at egui's time then, given the tab's session.
    right_click(&mut h, at, at, Modifiers::SHIFT);
    let clicks = secondary_seen(&seen);
    // The left button's stream is given nothing of it.
    let left = left_taken(&seen);
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    let seen_click = clicks[0];
    assert_eq!(seen_click.click.at_px, at_px(rect, at));
    assert!(seen_click.click.shift && !seen_click.click.ctrl);
    assert_eq!(Some(seen_click.click.frame), seen_click.serial);
    assert_eq!(seen_click.click.time, seen_click.time);
    assert_eq!(seen_click.session, h.state().tabs()[0].id);
    // Handed on once: the frame the view keeps holds none of it.
    assert!(view(&h).frame().unwrap().secondary.is_empty());
    // Ctrl (Cmd on macOS) without Shift.
    right_click(&mut h, at, at, Modifiers::COMMAND);
    let clicks = secondary_taken(&seen);
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert!(clicks[0].click.ctrl && !clicks[0].click.shift);
    // Let go 5 points from its press, 10 physical pixels (egui's click distance is 6
    // points): a click, where it was let go.
    let near = at + vec2(5.0, 0.0);
    right_click(&mut h, at, near, Modifiers::NONE);
    let clicks = secondary_taken(&seen);
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(clicks[0].click.at_px, at_px(rect, near));
    assert_eq!(clicks[0].click.at_px[0] - at_px(rect, at)[0], 10.0);
    assert!(!clicks[0].click.shift && !clicks[0].click.ctrl);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_right_drag_is_no_click_nor_is_a_right_click_while_the_pane_navigates() {
    use eframe::egui::{Modifiers, PointerButton, vec2};
    use silico_app::viewport::Navigation;
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let on = at + vec2(30.0, 0.0);
    let none = Modifiers::NONE;
    let right = PointerButton::Secondary;
    secondary_taken(&seen);
    // A right drag of 7 points, past egui's click distance, and its release: no click.
    let camera = view(&h).camera().clone();
    right_click(&mut h, at, at + vec2(7.0, 0.0), none);
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    // Nor does it navigate: no navigation takes the right button.
    assert_eq!(view(&h).camera(), &camera);
    // A right click, not moved, while the middle button pans: the pan's, none.
    h.hover_at(at);
    button_event(&h, at, PointerButton::Middle, true, none);
    h.hover_at(on);
    button_event(&h, on, right, true, none);
    button_event(&h, on, right, false, none);
    button_event(&h, on, PointerButton::Middle, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    assert_ne!(view(&h).camera(), &camera, "the middle button didn't pan");
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    // And while the Orbit tool's left drag orbits: none.
    view_mut(&mut h).tool = Some(Navigation::Orbit);
    step(&mut h);
    let camera = view(&h).camera().clone();
    secondary_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, PointerButton::Primary, true, none);
    h.hover_at(on);
    button_event(&h, on, right, true, none);
    button_event(&h, on, right, false, none);
    button_event(&h, on, PointerButton::Primary, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    assert_ne!(view(&h).camera(), &camera, "the tool didn't orbit");
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    // The tool still chosen, a right click with nothing dragging is handed on: a tool
    // takes the left drag alone.
    for _ in 0..30 {
        step(&mut h);
    }
    secondary_taken(&seen);
    right_click(&mut h, on, on, none);
    assert_eq!(secondary_taken(&seen).len(), 1);
    // And with no tool, the same right click well after: handed on, so none above was
    // missing for want of events.
    view_mut(&mut h).tool = None;
    for _ in 0..30 {
        step(&mut h);
    }
    secondary_taken(&seen);
    right_click(&mut h, on, on, none);
    let clicks = secondary_taken(&seen);
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(clicks[0].click.at_px, at_px(rect, on));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_right_click_on_the_view_cube_or_the_navigation_bar_is_theirs_not_the_canvass() {
    use eframe::egui::{Modifiers, Pos2, vec2};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let none = Modifiers::NONE;
    secondary_taken(&seen);
    // On the cube's Front face: its menu opens, and nothing is handed on.
    let front = h
        .get_by_role_and_label(Role::Group, "ViewCube")
        .get_by_label("Front")
        .rect();
    assert!(rect.contains(front.center()));
    right_click(&mut h, front.center(), front.center(), none);
    assert!(
        h.query_by_role_and_label(Role::RadioButton, "Perspective")
            .is_some(),
        "the cube's menu didn't open"
    );
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    // Closed by its own choice, the projection the view has.
    h.get_by_role_and_label(Role::RadioButton, "Orthographic")
        .click();
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(
        h.query_by_role_and_label(Role::RadioButton, "Perspective")
            .is_none(),
        "the cube's menu didn't close"
    );
    // On the bar's Orbit, and in its padding left of Orbit (6 points each side): none.
    let orbit = h
        .get_by_role_and_label(Role::Toolbar, "Navigation bar")
        .get_by_label("Orbit")
        .rect();
    let padding = Pos2::new(orbit.left() - 3.0, orbit.center().y);
    assert!(rect.contains(padding));
    secondary_taken(&seen);
    right_click(&mut h, orbit.center(), orbit.center(), none);
    right_click(&mut h, padding, padding, none);
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    assert_eq!(view(&h).tool, None);
    // And the same click on the canvas, well after: handed on, so none above was missing
    // for want of events.
    for _ in 0..30 {
        step(&mut h);
    }
    secondary_taken(&seen);
    let at = rect.center() - vec2(100.0, 0.0);
    right_click(&mut h, at, at, none);
    let clicks = secondary_taken(&seen);
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn several_right_clicks_in_one_frame_are_handed_on_in_their_order() {
    use eframe::egui::{Event, Modifiers, PointerButton, vec2};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let (a, b, c) = (at, at + vec2(40.0, 0.0), at + vec2(40.0, 30.0));
    let (d, dragged) = (at - vec2(40.0, 0.0), at - vec2(20.0, 0.0));
    let right = |pos, pressed, modifiers| Event::PointerButton {
        pos,
        button: PointerButton::Secondary,
        pressed,
        modifiers,
    };
    let none = Modifiers::NONE;
    secondary_taken(&seen);
    // Three clicks, the second with Shift and the third with Ctrl, and a right drag of 20
    // points between the first two, all in one frame.
    h.input_mut().events.extend([
        Event::PointerMoved(a),
        right(a, true, none),
        right(a, false, none),
        Event::PointerMoved(d),
        right(d, true, none),
        Event::PointerMoved(dragged),
        right(dragged, false, none),
        Event::PointerMoved(b),
        right(b, true, Modifiers::SHIFT),
        right(b, false, Modifiers::SHIFT),
        Event::PointerMoved(c),
        right(c, true, Modifiers::COMMAND),
        right(c, false, Modifiers::COMMAND),
    ]);
    step(&mut h);
    let clicks = secondary_taken(&seen);
    let places: Vec<[f32; 2]> = clicks.iter().map(|seen| seen.click.at_px).collect();
    assert_eq!(places, [a, b, c].map(|p| at_px(rect, p)), "{clicks:?}");
    let keys: Vec<(bool, bool)> = clicks
        .iter()
        .map(|seen| (seen.click.shift, seen.click.ctrl))
        .collect();
    assert_eq!(keys, [(false, false), (true, false), (false, true)]);
    // Each of that one frame and its one time.
    for seen in &clicks {
        assert_eq!(Some(seen.click.frame), seen.serial, "{seen:?}");
        assert_eq!(seen.click.time, seen.time, "{seen:?}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn right_clicks_leave_the_left_buttons_stream_as_it_was() {
    use eframe::egui::{Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{self, DoubleClick, Move, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let p = rect.center() - vec2(100.0, 0.0);
    let q = p + vec2(40.0, 0.0);
    let r = p + vec2(0.0, 40.0);
    let to = r + vec2(10.0, 5.0);
    let none = Modifiers::NONE;
    let (left, right) = (PointerButton::Primary, PointerButton::Secondary);
    let click = |h: &AppHarness, at, button| {
        button_event(h, at, button, true, none);
        button_event(h, at, button, false, none);
    };
    // The same left clicks and drag, with a right click a frame before each or none, every
    // event a frame of its own; the left button's events given, and the right clicks.
    type Given = Vec<(LeftKind, [f32; 2], bool, bool)>;
    let run = |h: &mut AppHarness, with_right: bool| -> (Given, usize) {
        for _ in 0..60 {
            step(h);
        }
        left_taken(&seen);
        // A left click a frame after a right one (egui, counting every button's clicks
        // together, calls it a double-click).
        h.hover_at(p);
        if with_right {
            click(h, p, right);
        }
        click(h, p, left);
        // A second past (egui's double-click delay is 0.3 s, and a triple's 0.6), a left
        // double-click a frame after a right click (to egui, a double and a triple).
        for _ in 0..60 {
            step(h);
        }
        h.hover_at(q);
        if with_right {
            click(h, q, right);
        }
        click(h, q, left);
        click(h, q, left);
        for _ in 0..60 {
            step(h);
        }
        // A right click in the middle of a left drag.
        h.hover_at(r);
        button_event(h, r, left, true, none);
        if with_right {
            click(h, r, right);
        }
        h.hover_at(to);
        button_event(h, to, left, false, none);
        for _ in 0..3 {
            step(h);
        }
        let clicks = secondary_seen(&seen).len();
        let given = left_taken(&seen)
            .iter()
            .map(|seen| {
                let e = seen.event;
                (e.kind, e.at_px, e.shift, e.ctrl)
            })
            .collect();
        (given, clicks)
    };
    let (alone, no_clicks) = run(&mut h, false);
    let (with_right, clicks) = run(&mut h, true);
    assert_eq!(no_clicks, 0);
    assert_eq!(clicks, 3, "the right clicks weren't handed on");
    let kinds: Vec<LeftKind> = alone.iter().map(|given| given.0).collect();
    assert_eq!(
        kinds,
        [
            Press,
            Release,
            Press,
            Release,
            Press,
            Release,
            DoubleClick,
            Press,
            Move,
            Release
        ]
    );
    assert_eq!(alone[6].1, at_px(rect, q));
    assert_eq!(with_right, alone);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_browse_fills_the_path_and_save_and_replace_go_on_as_typed() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, old) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    file_menu(&mut h, "Save As…", "Save example.silico as");
    browse(&mut h, &browser);
    assert_eq!(
        browser.asked(),
        [Ask {
            pick: Pick::Save,
            title: "Save example.silico as".to_owned(),
            filters: vec![browse_mod::DOCUMENTS],
            folder: Some(f.dir.path().to_path_buf()),
            file_name: Some("example.silico".to_owned()),
        }]
    );
    let frames = h.state().frames();
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), frames + 20);
    // Picked: the field holds it, and nothing is saved until Save.
    let copy = f.dir.path().join("copy.silico");
    browser.answer(Some(&copy));
    step_until(&mut h, "the field filled", |h| {
        path_value(h) == copy.to_str().unwrap()
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert!(!copy.exists());
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "the tab of the new file", |h| {
        h.state().tabs()[0].path == copy.to_str().unwrap()
    });
    assert!(copy.is_file());
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    // A file already there, picked: asked about as when typed, and Cancel writes nothing.
    let there = f.file("there.silico", "an older copy");
    step_until(&mut h, "saved", |h| !anywhere(h, "copy.silico *"));
    file_menu(&mut h, "Save As…", "Save copy.silico as");
    browse(&mut h, &browser);
    // Now the new file's folder and name.
    assert_eq!(browser.asked()[1].file_name.as_deref(), Some("copy.silico"));
    browser.answer(Some(&there));
    step_until(&mut h, "the field filled", |h| {
        path_value(h) == there.to_str().unwrap()
    });
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    let asked = format!("{} already exists. Replace it?", there.to_str().unwrap());
    step_until(&mut h, "the question", |h| anywhere(h, &asked));
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the question gone", |h| !anywhere(h, &asked));
    assert_eq!(std::fs::read_to_string(&there).unwrap(), "an older copy");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn exports_browse_asks_its_formats_fills_the_path_and_the_format_follows_it() {
    use silico_core::command::export::{Export, ExportFormat};
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, _) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    open_export(&mut h);
    browse(&mut h, &browser);
    // 3MF chosen: its filter first.
    assert_eq!(
        browser.asked(),
        [Ask {
            pick: Pick::Save,
            title: "Export example.silico".to_owned(),
            filters: vec![
                browse_mod::Filter {
                    name: "3MF",
                    extensions: &["3mf"]
                },
                browse_mod::Filter {
                    name: "STEP",
                    extensions: &["step", "stp"]
                },
            ],
            folder: Some(f.dir.path().to_path_buf()),
            file_name: Some("example.3mf".to_owned()),
        }]
    );
    let frames = h.state().frames();
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), frames + 20);
    // A STEP name picked while 3MF is chosen: the field holds it, and STEP is chosen (no
    // tolerance asked), nothing exported until Export.
    let out = f.dir.path().join("sub").join("out.stp");
    std::fs::create_dir(f.dir.path().join("sub")).unwrap();
    browser.answer(Some(&out));
    step_until(&mut h, "the field filled", |h| {
        path_value(h) == out.to_str().unwrap()
    });
    assert!(
        h.query_by_role_and_label(Role::TextInput, "Tolerance")
            .is_none()
    );
    assert!(h.state().tabs()[0].exports.is_empty());
    press(&mut h, "Export");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert_eq!(
        h.state().tabs()[0].exports,
        [Export {
            format: ExportFormat::Step,
            path: out.clone(),
            replace: false,
            features: None,
        }]
    );
    assert!(out.is_file());
    // STEP chosen, browsing again: STEP's filter first.
    open_export(&mut h);
    press(&mut h, "STEP");
    browse(&mut h, &browser);
    // The document's folder, not the folder last picked in (`sub`).
    assert_eq!(browser.asked()[1].folder.as_deref(), Some(f.dir.path()));
    assert_eq!(browser.asked()[1].filters[0].name, "STEP");
    assert_eq!(
        browser.asked()[1].file_name.as_deref(),
        Some("example.step")
    );
    // A 3MF name picked: 3MF chosen again, its tolerance asked.
    let mesh = f.dir.path().join("out.3MF");
    browser.answer(Some(&mesh));
    step_until(&mut h, "the field filled", |h| {
        path_value(h) == mesh.to_str().unwrap()
    });
    assert!(
        h.query_by_role_and_label(Role::TextInput, "Tolerance")
            .is_some()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn news_browse_asks_where_to_save_and_starts_in_the_last_folder_picked() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    file_menu(&mut h, "New…", "New document");
    browse(&mut h, &browser);
    assert_eq!(
        browser.asked(),
        [Ask {
            pick: Pick::Save,
            title: "New document".to_owned(),
            filters: vec![browse_mod::DOCUMENTS],
            folder: None,
            file_name: None,
        }]
    );
    let sub = f.dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let made = sub.join("new.silico");
    browser.answer(Some(&made));
    step_until(&mut h, "the field filled", |h| {
        path_value(h) == made.to_str().unwrap()
    });
    // Filled, not made: Create makes it.
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(!made.exists());
    assert!(h.state().tabs().is_empty());
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| !anywhere(h, "New document"));
    // No document open: the next starts in the folder last picked in.
    file_menu(&mut h, "Open…", "Open document");
    browse(&mut h, &browser);
    assert_eq!(browser.asked()[1].folder.as_deref(), Some(sub.as_path()));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_pick_answering_after_its_dialog_closed_changes_nothing() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    file_menu(&mut h, "Open…", "Open document");
    browse(&mut h, &browser);
    // The app's dialog cancelled while the native one is open.
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| !anywhere(h, "Open document"));
    let path = f.file("late.silico", EXAMPLE);
    browser.answer(Some(&path));
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(h.state().tabs().is_empty());
    // Nor does it go to the next question asked: a new Open's field is as it was.
    file_menu(&mut h, "Open…", "Open document");
    assert_eq!(path_value(&h), "");
    assert!(!browse_button(&h).accesskit_node().is_disabled());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_opens_pick_answering_while_save_as_is_asked_neither_opens_nor_fills_it() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, path) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    // Open, a path typed, browsing; then cancelled, its native dialog still open.
    file_menu(&mut h, "Open…", "Open document");
    h.get_by_role(Role::TextInput).focus();
    step(&mut h);
    h.get_by_role(Role::TextInput).type_text("typed.silico");
    step(&mut h);
    browse(&mut h, &browser);
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| !anywhere(h, "Open document"));
    // Save As asked, and only then Open's pick answers.
    file_menu(&mut h, "Save As…", "Save example.silico as");
    let late = f.file("late.silico", EXAMPLE);
    browser.answer(Some(&late));
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(h.state().tabs()[0].path, path.to_str().unwrap());
    assert!(anywhere(&h, "Save example.silico as"));
    assert_eq!(path_value(&h), "");
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert!(!browse_button(&h).accesskit_node().is_disabled());
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| {
        !anywhere(h, "Save example.silico as")
    });
    // Open's field holds what was typed in it, and nothing more was opened.
    file_menu(&mut h, "Open…", "Open document");
    assert_eq!(path_value(&h), "typed.silico");
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_exports_pick_answering_while_new_is_asked_neither_fills_it_nor_exports() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, _) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    open_export(&mut h);
    let before = path_value(&h);
    browse(&mut h, &browser);
    press(&mut h, "Cancel");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    // New asked, and only then Export's pick answers, a STEP name.
    file_menu(&mut h, "New…", "New document");
    let late = f.dir.path().join("late.step");
    browser.answer(Some(&late));
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(anywhere(&h, "New document"));
    assert_eq!(path_value(&h), "");
    assert!(!browse_button(&h).accesskit_node().is_disabled());
    assert_eq!(h.state().tabs().len(), 1);
    assert!(h.state().tabs()[0].exports.is_empty());
    assert!(!late.exists());
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| !anywhere(h, "New document"));
    // Export again: its path and format as they began, 3MF's tolerance asked.
    open_export(&mut h);
    assert_eq!(path_value(&h), before);
    assert!(
        h.query_by_role_and_label(Role::TextInput, "Tolerance")
            .is_some()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_exports_pick_answering_while_its_export_runs_changes_neither_path_nor_format() {
    use silico_core::command::export::{Export, ExportFormat};
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, path) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let hold = f.kernel.hold(is_step);
    open_export(&mut h);
    press(&mut h, "STEP");
    let before = path_value(&h);
    // Browsing, and Export pressed meanwhile: the export runs, held.
    browse(&mut h, &browser);
    press(&mut h, "Export");
    assert!(f.kernel.wait_running(1, WAIT));
    step_until(&mut h, "running", |h| anywhere(h, "Exporting…"));
    // A 3MF name picked now: neither the path nor the format follows it.
    let picked = f.dir.path().join("picked.3mf");
    browser.answer(Some(&picked));
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(anywhere(&h, "Exporting…"));
    assert_eq!(path_value(&h), before);
    assert!(
        h.query_by_role_and_label(Role::TextInput, "Tolerance")
            .is_none()
    );
    hold.release();
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    assert_eq!(
        h.state().tabs()[0].exports,
        [Export {
            format: ExportFormat::Step,
            path: path.with_extension("step"),
            replace: false,
            features: None,
        }]
    );
    assert!(!picked.exists());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancel_picks_no_folder() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    file_menu(&mut h, "Open…", "Open document");
    browse(&mut h, &browser);
    browser.answer(None);
    step_until(&mut h, "Browse… on again", |h| {
        !browse_button(h).accesskit_node().is_disabled()
    });
    // No document open and nothing picked: the platform's own start again.
    browse(&mut h, &browser);
    assert_eq!(browser.asked().len(), 2);
    assert_eq!(browser.asked()[1].folder, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn browse_while_a_dialog_is_open_is_off_and_says_why_when_hovered() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    file_menu(&mut h, "Open…", "Open document");
    let why = "A file dialog is already open";
    // On: hovered, it says nothing.
    browse_button(&h).hover();
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(!anywhere(&h, why));
    browse(&mut h, &browser);
    // Drawn off from the frame after the one it opened in.
    step(&mut h);
    assert!(browse_button(&h).accesskit_node().is_disabled());
    browse_button(&h).hover();
    step_until(&mut h, "why it's off", |h| anywhere(h, why));
    // Answered: on again, and saying nothing.
    browser.answer(None);
    step_until(&mut h, "Browse… on again", |h| {
        !browse_button(h).accesskit_node().is_disabled()
    });
    browse_button(&h).hover();
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(!anywhere(&h, why));
    assert_eq!(never_waits::waits(), 0);
}

#[cfg(unix)]
#[test]
fn a_pick_whose_path_isnt_utf8_is_said_and_neither_opens_nor_fills() {
    use std::os::unix::ffi::OsStrExt;
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    file_menu(&mut h, "Open…", "Open document");
    browse(&mut h, &browser);
    let mut bytes = f.dir.path().as_os_str().as_bytes().to_vec();
    bytes.extend_from_slice(b"/\xff.silico");
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
    browser.answer(Some(&path));
    step_until(&mut h, "said", |h| h.state().message().is_some());
    assert_eq!(
        h.state().message(),
        Some(format!("{} can't be used: its path isn't UTF-8", path.display()).as_str())
    );
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(h.state().tabs().is_empty());
    assert!(anywhere(&h, "Open document"));
    assert_eq!(path_value(&h), "");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_opens_pick_refused_at_once_asks_for_the_frame_that_says_so() {
    // Every session the process may hold already open: the pick's open is refused at once,
    // as it's asked, so no waker asks for the frame that takes the refusal; the frame that
    // took the pick asks for it.
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    for n in 0..silico_core::command::MAX_SESSIONS {
        let path = f.dir.path().join(format!("held{n}.silico"));
        f.sessions.open(&path, true).wait().session.unwrap();
    }
    file_menu(&mut h, "Open…", "Open document");
    browse(&mut h, &browser);
    quiet_wakes(&mut h);
    idle(&mut h);
    let path = f.file("picked.silico", EXAMPLE);
    browser.answer(Some(&path));
    let woken = h.state().wakes();
    // The frame the answer's waker asked for (egui gives each such request two frames): it
    // takes the pick, the dialog closes, and the open is refused, not yet taken.
    step(&mut h);
    assert!(!anywhere(&h, "Open document"));
    assert_eq!(h.state().message(), None);
    // Asked for at once by the frame before, nothing having woken it: the refusal said.
    step(&mut h);
    assert!(asked_at_once(&h));
    assert_eq!(h.state().wakes(), woken);
    let said = format!("{} sessions are open", silico_core::command::MAX_SESSIONS);
    assert!(
        h.state().message().is_some_and(|m| m.contains(&said)),
        "{:?}",
        h.state().message()
    );
    // And then nothing more at once.
    step(&mut h);
    assert!(!asked_at_once(&h));
    assert!(h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_themed_app_draws_its_panels_in_the_themes_colours_on_lavapipe() {
    on_lavapipe(
        "the_themed_app_draws_its_panels_in_the_themes_colours_on_lavapipe",
        || {
            let f = fixture();
            meshes_a_real_box(&f);
            let state = lavapipe_render_state();
            let present = silico_app::renderer::on_eframe(&state).expect("no renderer");
            let app = App::new(Arc::clone(&f.sessions)).presenting(Box::new(present));
            let mut h = {
                let _marked = never_waits::marked();
                Harness::builder()
                    .with_size((1280.0, 800.0))
                    .with_step_dt(1.0 / 60.0)
                    .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                        state.clone(),
                    ))
                    .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
            };
            // As main.rs does, once at startup.
            silico_app::theme::apply(&h.ctx);
            h.set_render_every_step(true);
            let path = f.file("box.silico", A_BOX);
            h.state_mut().open(path.to_str().unwrap(), false);
            meshes_settled(&mut h);
            select(&mut h, "Block");
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let ppp = h.ctx.pixels_per_point();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            // The browser's empty space, left of the viewport and above the timeline: the
            // panels' Gray.100 (DESIGN.md, *Tokens*).
            let viewport = the_viewport(&h);
            let browser = viewport.left_bottom() + eframe::egui::vec2(-20.0, -20.0);
            assert_eq!(pixel(browser), [0x0d, 0x0d, 0x0d, 0xff]);
            // The selected feature's timeline entry: Blue.500 (styles.css, `.tl-item.selected`)
            // at its left edge, inside its fill, clear of a focus ring's 1 point, and short
            // of its text.
            let entry = h
                .get_by_role_and_label(Role::Pane, "Timeline")
                .get_by_label("Block")
                .rect();
            let inside = eframe::egui::pos2(entry.left() + 2.5, entry.center().y);
            assert_eq!(pixel(inside), [0x18, 0x6a, 0xdd, 0xff], "{entry:?}");
        },
    );
}

#[test]
fn a_right_press_in_one_tab_let_go_in_another_is_no_click_in_either() {
    use eframe::egui::{Modifiers, PointerButton, vec2};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let (first, second) = two_tabs_first_shown(&f, &mut h, &seen);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let right = PointerButton::Secondary;
    let none = Modifiers::NONE;
    secondary_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, right, true, none);
    step(&mut h);
    // The second tab chosen with no button (through the accessibility tree), and the
    // release there, well within egui's click duration.
    let frames = h.state().frames();
    h.get_by_role_and_label(Role::Button, "other.silico")
        .click_accesskit();
    step_until(&mut h, "the second shown", |h| {
        left_shown(h) == Some(second)
    });
    step(&mut h);
    assert!(h.state().frames() - frames < 20, "too slow to be a click");
    button_event(&h, at, right, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    // Both tabs were shown and watched while it was held.
    let shown: Vec<_> = seen
        .lock()
        .unwrap()
        .frames
        .iter()
        .map(|(_, frame, _)| frame.session)
        .collect();
    assert!(
        shown.contains(&first) && shown.contains(&second),
        "{shown:?}"
    );
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    // A click of its own, in the tab shown: handed on to it.
    right_click(&mut h, at, at, none);
    let clicks = secondary_taken(&seen);
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(clicks[0].session, second);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_right_press_is_no_click_once_another_button_goes_between_the_focus_goes_or_it_strays() {
    use eframe::egui::{Event, Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let at = rect.center() - vec2(100.0, 0.0);
    let none = Modifiers::NONE;
    let (left, right) = (PointerButton::Primary, PointerButton::Secondary);
    let rest = |h: &mut AppHarness| {
        for _ in 0..60 {
            step(h);
        }
    };
    left_taken(&seen);
    // A left press and release between the right press and its release: no right click,
    // and the left click is handed on.
    h.hover_at(at);
    button_event(&h, at, right, true, none);
    button_event(&h, at, left, true, none);
    button_event(&h, at, left, false, none);
    button_event(&h, at, right, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let clicks = secondary_seen(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    let lefts = left_taken(&seen);
    assert_eq!(kinds(&lefts), [Press, Release]);
    assert!(lefts.iter().all(|seen| seen.event.at_px == at_px(rect, at)));
    rest(&mut h);
    // The window's focus going between them: none.
    left_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, right, true, none);
    h.event(Event::WindowFocused(false));
    button_event(&h, at, right, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    h.event(Event::WindowFocused(true));
    rest(&mut h);
    // Gone 10 points away and back to the press before the release: none.
    secondary_taken(&seen);
    h.hover_at(at);
    button_event(&h, at, right, true, none);
    h.hover_at(at + vec2(10.0, 0.0));
    h.hover_at(at);
    button_event(&h, at, right, false, none);
    for _ in 0..3 {
        step(&mut h);
    }
    let clicks = secondary_taken(&seen);
    assert!(clicks.is_empty(), "{clicks:?}");
    rest(&mut h);
    // The same press and release with none of them: a click, so none above was missing
    // for want of events.
    secondary_taken(&seen);
    right_click(&mut h, at, at, none);
    assert_eq!(secondary_taken(&seen).len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

// ---- The start page (docs/design/m5-ui/DESIGN.md, *Start page or blank canvas?*) ----

impl Fixture {
    /// The app on these sessions as `make` makes it, in a harness, with nothing open.
    fn app_made(&self, make: impl FnOnce(App<Fake>) -> App<Fake>) -> AppHarness {
        let app = make(App::new(Arc::clone(&self.sessions)));
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    }

    /// Where these tests keep the list of recent documents: in a folder not yet made.
    fn recents_file(&self) -> PathBuf {
        self.dir.path().join("app-data").join("recent-documents")
    }

    /// The app keeping its recent documents in [`recents_file`](Self::recents_file), the
    /// list read.
    fn remembering(&self) -> AppHarness {
        let file = self.recents_file();
        let mut h = self.app_made(|app| app.remembering(Recents::at(file)));
        recents_read(&mut h);
        h
    }
}

/// Frames stepped until the app has heard the list of recent documents read.
fn recents_read(h: &mut AppHarness) {
    step_until(h, "the recent documents read", |h| {
        h.state().recents().is_some_and(Recents::is_read)
    });
}

/// Whether `node` is inside the pane labelled `pane`.
fn in_pane(node: &egui_kittest::Node<'_>, pane: &str) -> bool {
    let mut at = node.parent();
    while let Some(node) = at {
        let shown = node.accesskit_node();
        if shown.role() == Role::Pane && shown.label().as_deref() == Some(pane) {
            return true;
        }
        at = node.parent();
    }
    false
}

/// Whether `node` is shown selected (toggled on).
fn toggled(node: &egui_kittest::Node<'_>) -> Option<bool> {
    node.accesskit_node()
        .toggled()
        .map(|toggled| toggled == eframe::egui::accesskit::Toggled::True)
}

/// The start page.
fn start_page(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::Pane, "Start")
}

/// Whether the start page is shown.
fn start_shown(h: &AppHarness) -> bool {
    h.query_by_role_and_label(Role::Pane, "Start").is_some()
}

/// The app bar's Home (the viewport's view cube has a Home of its own).
fn home_button(h: &AppHarness) -> egui_kittest::Node<'_> {
    let mut found = h
        .query_all_by_role_and_label(Role::Button, "Home")
        .filter(|node| !in_pane(node, "Viewport"));
    let home = found.next().expect("the app bar's Home");
    assert!(found.next().is_none(), "two Homes outside the viewport");
    home
}

/// The start page's Recent, each document's name in order, and each one's description.
fn recent_rows(h: &AppHarness) -> Vec<(String, String)> {
    start_page(h)
        .get_by_role_and_label(Role::List, "Recent")
        .query_all_by_role(Role::Button)
        .map(|node| {
            let node = node.accesskit_node();
            (
                node.label().unwrap_or_default(),
                node.description().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// The start page's Recent, each document's name in order.
fn recent_names(h: &AppHarness) -> Vec<String> {
    recent_rows(h).into_iter().map(|(name, _)| name).collect()
}

/// `path` opened, its tab waited for.
fn open_and_wait(h: &mut AppHarness, path: &Path) {
    let name = path.file_name().unwrap().to_str().unwrap().to_owned();
    h.state_mut().open(path.to_str().unwrap(), false);
    step_until(h, "its tab", |h| {
        h.state().tabs().iter().any(|tab| tab.name() == name)
    });
}

/// The text the frame just stepped copied, if it copied any.
fn copied(h: &AppHarness) -> Option<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .find_map(|command| match command {
            eframe::egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
}

#[test]
fn with_no_document_the_start_page_shows_new_open_recent_and_copy_mcp_config() {
    let f = fixture();
    let mut h = f.remembering();
    let start = start_page(&h);
    for button in ["New part", "Open…", "Copy MCP config"] {
        assert!(
            start
                .query_by_role_and_label(Role::Button, button)
                .is_some(),
            "{button}"
        );
    }
    assert!(
        start
            .query_by_role_and_label(Role::List, "Recent")
            .is_some()
    );
    assert!(
        start
            .query_by_label("Recent".to_uppercase().as_str())
            .is_some()
    );
    assert!(
        start
            .query_by_label("No recent documents yet: each you open or make is listed here.")
            .is_some()
    );
    // Nothing else between the bars: no panes, no toolbar.
    for pane in ["Browser", "Viewport", "Properties", "Timeline"] {
        assert!(
            h.query_by_role_and_label(Role::Pane, pane).is_none(),
            "{pane}"
        );
    }
    assert!(h.query_by_role_and_label(Role::Toolbar, "CREATE").is_none());
    // Home is the page shown.
    assert_eq!(toggled(&home_button(&h)), Some(true));
    // New part asks where the new document goes, as File > New… does.
    start_page(&h)
        .get_by_role_and_label(Role::Button, "New part")
        .click();
    step_until(&mut h, "New's dialog", |h| anywhere(h, "New document"));
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "it gone", |h| !anywhere(h, "New document"));
    assert!(h.state().tabs().is_empty());
    assert!(start_shown(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_frames_left_events_and_right_clicks_are_numbered_in_their_order_together() {
    use eframe::egui::{Event, Modifiers, PointerButton, vec2};
    use silico_app::viewport::LeftKind::{DoubleClick, Press, Release};
    let f = fixture();
    let (mut h, seen) = f.overlaid(EXAMPLE);
    let rect = the_viewport(&h);
    let a = rect.center() - vec2(100.0, 0.0);
    let b = a + vec2(40.0, 0.0);
    let button = |pos, button, pressed| Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let (left, right) = (PointerButton::Primary, PointerButton::Secondary);
    for _ in 0..60 {
        step(&mut h);
    }
    left_taken(&seen);
    // In one frame: a left click, a right click, and a second left click, the double.
    h.input_mut().events.extend([
        Event::PointerMoved(a),
        button(a, left, true),
        button(a, left, false),
        Event::PointerMoved(b),
        button(b, right, true),
        button(b, right, false),
        Event::PointerMoved(a),
        button(a, left, true),
        button(a, left, false),
    ]);
    step(&mut h);
    let clicks = secondary_seen(&seen);
    let lefts = left_taken(&seen);
    assert_eq!(kinds(&lefts), [Press, Release, Press, Release, DoubleClick]);
    let left_seqs: Vec<u32> = lefts.iter().map(|seen| seen.event.seq).collect();
    assert_eq!(left_seqs, [0, 1, 3, 4, 5]);
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(clicks[0].click.seq, 2);
    assert_eq!(clicks[0].click.at_px, at_px(rect, b));
    // All of one frame.
    assert!(
        lefts
            .iter()
            .all(|seen| seen.event.frame == clicks[0].click.frame)
    );
    // The next frame's are numbered from 0 again.
    h.hover_at(b);
    button_event(&h, b, right, true, Modifiers::NONE);
    button_event(&h, b, right, false, Modifiers::NONE);
    for _ in 0..3 {
        step(&mut h);
    }
    let clicks = secondary_taken(&seen);
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(clicks[0].click.seq, 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_start_pages_open_asks_as_file_open_does_and_its_browse_opens_the_pick() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let path = f.file("example.silico", EXAMPLE);
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    start_page(&h)
        .get_by_role_and_label(Role::Button, "Open…")
        .click();
    step_until(&mut h, "Open's dialog", |h| anywhere(h, "Open document"));
    browse(&mut h, &browser);
    assert_eq!(browser.asked()[0].pick, Pick::Open);
    browser.answer(Some(&path));
    step_until(&mut h, "its tab", |h| !h.state().tabs().is_empty());
    assert_eq!(h.state().tabs()[0].path, path.to_str().unwrap());
    step_until(&mut h, "the start page left", |h| !start_shown(h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn command_n_and_command_o_ask_for_new_and_open_from_the_start_page() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    for (key, heading) in [
        (eframe::egui::Key::N, "New document"),
        (eframe::egui::Key::O, "Open document"),
    ] {
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, key);
        step_until(&mut h, heading, |h| anywhere(h, heading));
        h.key_press(eframe::egui::Key::Escape);
        step_until(&mut h, "it gone", |h| !anywhere(h, heading));
    }
    assert!(h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn opening_a_document_leaves_the_start_page_and_home_returns_to_it() {
    let f = fixture();
    let path = f.file("example.silico", EXAMPLE);
    let mut h = f.app();
    step(&mut h);
    assert!(start_shown(&h));
    open_and_wait(&mut h, &path);
    step(&mut h);
    assert!(!start_shown(&h));
    assert!(h.query_by_role_and_label(Role::Pane, "Browser").is_some());
    assert_eq!(toggled(&home_button(&h)), Some(false));
    assert_eq!(
        toggled(&h.get_by_role_and_label(Role::Button, "example.silico")),
        Some(true)
    );
    // Home: the start page, over the open document, which stays open.
    home_button(&h).click();
    step(&mut h);
    step(&mut h);
    assert!(start_shown(&h));
    assert!(h.query_by_role_and_label(Role::Pane, "Browser").is_none());
    assert_eq!(toggled(&home_button(&h)), Some(true));
    assert_eq!(
        toggled(&h.get_by_role_and_label(Role::Button, "example.silico")),
        Some(false)
    );
    assert_eq!(h.state().tabs().len(), 1);
    // Its tab: the document again.
    h.get_by_role_and_label(Role::Button, "example.silico")
        .click();
    step(&mut h);
    step(&mut h);
    assert!(!start_shown(&h));
    assert!(h.query_by_role_and_label(Role::Pane, "Browser").is_some());
    assert_eq!(
        toggled(&h.get_by_role_and_label(Role::Button, "example.silico")),
        Some(true)
    );
    // Home again, then another document opened: it's shown, and the start page left.
    home_button(&h).click();
    step(&mut h);
    step(&mut h);
    assert!(start_shown(&h));
    let other = f.file("other.silico", EXAMPLE);
    open_and_wait(&mut h, &other);
    step(&mut h);
    assert!(!start_shown(&h));
    assert_eq!(
        toggled(&h.get_by_role_and_label(Role::Button, "other.silico")),
        Some(true)
    );
    // The last tab closed: the start page, with no Home pressed.
    for name in ["other.silico", "example.silico"] {
        h.get_by_label(&format!("Close {name}")).click();
        step_until(&mut h, "the tab closed", |h| {
            h.state().tabs().iter().all(|tab| tab.name() != name)
        });
    }
    step(&mut h);
    assert!(start_shown(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn recent_lists_the_last_opened_newest_first_each_once_and_a_new_app_reads_them() {
    let f = fixture();
    let one = f.file("one.silico", EXAMPLE);
    let two = f.file("two.silico", EXAMPLE);
    let mut h = f.remembering();
    open_and_wait(&mut h, &one);
    open_and_wait(&mut h, &two);
    // One closed and opened again: first now, and listed once.
    h.get_by_label("Close one.silico").click();
    step_until(&mut h, "one closed", |h| h.state().tabs().len() == 1);
    open_and_wait(&mut h, &one);
    home_button(&h).click();
    step_until(&mut h, "Recent, one first", |h| {
        start_shown(h) && recent_names(h) == ["one.silico", "two.silico"]
    });
    let (one, two) = (one.to_str().unwrap(), two.to_str().unwrap());
    assert_eq!(
        recent_rows(&h),
        [
            ("one.silico".to_owned(), one.to_owned()),
            ("two.silico".to_owned(), two.to_owned())
        ]
    );
    // The file, as recent.rs's form has it: a header, then a path a line, newest first.
    assert_eq!(
        std::fs::read_to_string(f.recents_file()).unwrap(),
        format!("silico recent-documents 1\n{one}\n{two}\n")
    );
    // A recent document already open: its tab shown, not opened again.
    start_page(&h)
        .get_by_role_and_label(Role::Button, "two.silico")
        .click();
    step(&mut h);
    step(&mut h);
    assert!(!start_shown(&h));
    assert_eq!(
        toggled(&h.get_by_role_and_label(Role::Button, "two.silico")),
        Some(true)
    );
    assert_eq!(h.state().tabs().len(), 2);
    assert_eq!(h.state().message(), None);
    for name in ["one.silico", "two.silico"] {
        h.get_by_label(&format!("Close {name}")).click();
        step_until(&mut h, "the tab closed", |h| {
            h.state().tabs().iter().all(|tab| tab.name() != name)
        });
    }
    // Another app on the same file lists the same.
    let mut again = f.remembering();
    step(&mut again);
    assert_eq!(recent_names(&again), ["one.silico", "two.silico"]);
    // A recent document clicked opens it.
    start_page(&again)
        .get_by_role_and_label(Role::Button, "two.silico")
        .click();
    step_until(&mut again, "two's tab", |h| {
        h.state()
            .tabs()
            .iter()
            .any(|tab| tab.name() == "two.silico")
    });
    assert_eq!(again.state().tabs()[0].path, two);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_recent_document_no_longer_there_is_said_and_dropped_from_the_list() {
    let f = fixture();
    let kept = f.file("kept.silico", EXAMPLE);
    let gone = f.dir.path().join("gone.silico");
    let (kept_at, gone_at) = (kept.to_str().unwrap(), gone.to_str().unwrap());
    std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
    std::fs::write(
        f.recents_file(),
        format!("silico recent-documents 1\n{gone_at}\n{kept_at}\n"),
    )
    .unwrap();
    let mut h = f.remembering();
    // Nothing at gone's path when the list was looked through: marked so, once looked.
    let want = [
        ("gone.silico".to_owned(), format!("{gone_at}, not found")),
        ("kept.silico".to_owned(), kept_at.to_owned()),
    ];
    step_until(&mut h, "gone marked", |h| recent_rows(h) == want);
    let folder = f.dir.path().display().to_string();
    start_page(&h)
        .get_by_role_and_label(Role::Button, "gone.silico")
        .click();
    let said =
        format!("gone.silico isn't in {folder} any more, so it's no longer listed in Recent");
    step_until(&mut h, "gone said and dropped", |h| {
        h.state().message() == Some(said.as_str()) && recent_names(h) == ["kept.silico"]
    });
    assert!(anywhere(&h, &said));
    assert!(h.state().tabs().is_empty());
    assert_eq!(
        std::fs::read_to_string(f.recents_file()).unwrap(),
        format!("silico recent-documents 1\n{kept_at}\n")
    );
    // Kept goes from disk after the list was read: its open is refused, and it's dropped.
    std::fs::remove_file(&kept).unwrap();
    start_page(&h)
        .get_by_role_and_label(Role::Button, "kept.silico")
        .click();
    let said =
        format!("kept.silico isn't in {folder} any more, so it's no longer listed in Recent");
    step_until(&mut h, "kept said and dropped", |h| {
        h.state().message() == Some(said.as_str()) && recent_names(h).is_empty()
    });
    assert_eq!(
        std::fs::read_to_string(f.recents_file()).unwrap(),
        "silico recent-documents 1\n"
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_recent_document_refused_for_another_reason_stays_listed() {
    let f = fixture();
    // A folder: there, but no document, so its open is refused.
    let broken = f.dir.path().join("broken.silico");
    std::fs::create_dir(&broken).unwrap();
    let at = broken.to_str().unwrap().to_owned();
    std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
    std::fs::write(
        f.recents_file(),
        format!("silico recent-documents 1\n{at}\n"),
    )
    .unwrap();
    let mut h = f.remembering();
    step(&mut h);
    start_page(&h)
        .get_by_role_and_label(Role::Button, "broken.silico")
        .click();
    step_until(&mut h, "the refusal said", |h| {
        h.state().message().is_some()
    });
    assert!(!h.state().message().unwrap().contains("Recent"));
    // Another document opened after it: once that's listed, the list's thread has taken
    // the refusal's check too, asked before it.
    let other = f.file("other.silico", EXAMPLE);
    let other_at = other.to_str().unwrap().to_owned();
    open_and_wait(&mut h, &other);
    let listed = |h: &AppHarness| -> Vec<String> {
        h.state()
            .recents()
            .unwrap()
            .list()
            .iter()
            .map(|recent| recent.path.clone())
            .collect()
    };
    step_until(&mut h, "other listed", |h| {
        listed(h).first() == Some(&other_at)
    });
    assert_eq!(listed(&h), [other_at.clone(), at.clone()]);
    assert_eq!(
        std::fs::read_to_string(f.recents_file()).unwrap(),
        format!("silico recent-documents 1\n{other_at}\n{at}\n")
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_corrupt_recents_file_lists_nothing_and_the_next_open_replaces_it() {
    let f = fixture();
    let listed = f.file("listed.silico", EXAMPLE);
    std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
    // Its paths are real, but its header isn't the form's.
    std::fs::write(
        f.recents_file(),
        format!("silico recent 1\n{}\n", listed.to_str().unwrap()),
    )
    .unwrap();
    let mut h = f.remembering();
    step(&mut h);
    assert!(recent_names(&h).is_empty());
    assert!(
        start_page(&h)
            .query_by_label("No recent documents yet: each you open or make is listed here.")
            .is_some()
    );
    let opened = f.file("opened.silico", EXAMPLE);
    open_and_wait(&mut h, &opened);
    home_button(&h).click();
    step_until(&mut h, "Recent", |h| {
        start_shown(h) && recent_names(h) == ["opened.silico"]
    });
    assert_eq!(
        std::fs::read_to_string(f.recents_file()).unwrap(),
        format!("silico recent-documents 1\n{}\n", opened.to_str().unwrap())
    );
    // Bytes that aren't text: nothing either.
    std::fs::write(f.recents_file(), b"silico recent-documents 1\n\xff\n").unwrap();
    let mut again = f.remembering();
    step(&mut again);
    assert!(recent_names(&again).is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_recents_file_is_read_off_the_ui_thread_and_an_open_meanwhile_comes_first() {
    let f = fixture();
    let listed = f.file("listed.silico", EXAMPLE);
    let opened = f.file("opened.silico", EXAMPLE);
    std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
    std::fs::write(
        f.recents_file(),
        format!("silico recent-documents 1\n{}\n", listed.to_str().unwrap()),
    )
    .unwrap();
    // The read held until released, saying which thread it ran on.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let reader = Arc::new(Mutex::new(None));
    let read_on = Arc::clone(&reader);
    let read: silico_app::recent::Read = Box::new(move |file| {
        *read_on.lock().unwrap() = Some(std::thread::current().id());
        released.recv_timeout(WAIT).ok()?;
        std::fs::read(file).ok()
    });
    let file = f.recents_file();
    let mut h = f.app_made(|app| app.remembering(Recents::reading(file, read)));
    let frames = h.state().frames();
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(h.state().frames() >= frames + 20);
    step_until(&mut h, "the read started", |_| {
        reader.lock().unwrap().is_some()
    });
    assert_ne!(*reader.lock().unwrap(), Some(std::thread::current().id()));
    assert!(
        start_page(&h)
            .query_by_label("Reading the list of recent documents…")
            .is_some()
    );
    // An open while it's held: the window goes on, and the open waits for the list.
    open_and_wait(&mut h, &opened);
    home_button(&h).click();
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(start_shown(&h));
    assert!(!h.state().recents().unwrap().is_read());
    release.send(()).unwrap();
    step_until(&mut h, "both listed", |h| {
        recent_names(h) == ["opened.silico", "listed.silico"]
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn copy_mcp_config_copies_the_config_docs_mcp_md_gives() {
    let f = fixture();
    let mut h = f.app_made(|app| {
        app.serving_with("/opt/silico/bin/silico".to_owned())
            .documents_folder(None)
    });
    step(&mut h);
    // No folder to name: the note beside the button says to add one.
    assert!(anywhere(&h, silico_app::start::NO_WORKSPACE_NOTE));
    start_page(&h)
        .get_by_role_and_label(Role::Button, "Copy MCP config")
        .click();
    step(&mut h);
    // docs/mcp.md's form, written out by hand: no folder known, so no workspace.
    assert_eq!(
        copied(&h).as_deref(),
        Some(
            r#"{
  "mcpServers": {
    "silico": {
      "command": "/opt/silico/bin/silico",
      "args": ["serve"]
    }
  }
}
"#
        )
    );
    step(&mut h);
    assert_eq!(copied(&h), None);
    // A document open: its folder is the workspace, and the note goes.
    let path = f.file("example.silico", EXAMPLE);
    open_and_wait(&mut h, &path);
    home_button(&h).click();
    step_until(&mut h, "the start page", start_shown);
    start_page(&h)
        .get_by_role_and_label(Role::Button, "Copy MCP config")
        .click();
    step(&mut h);
    let folder = f.dir.path().to_str().unwrap();
    assert_eq!(
        copied(&h),
        Some(format!(
            r#"{{
  "mcpServers": {{
    "silico": {{
      "command": "/opt/silico/bin/silico",
      "args": ["serve", "--workspace", "{folder}"]
    }}
  }}
}}
"#
        ))
    );
    assert!(!anywhere(&h, silico_app::start::NO_WORKSPACE_NOTE));
    assert_eq!(never_waits::waits(), 0);
}

/// The MCP config Copy MCP config copies for a server of `folder`, as docs/mcp.md gives it,
/// written out by hand.
fn config_serving(folder: &str) -> String {
    format!(
        r#"{{
  "mcpServers": {{
    "silico": {{
      "command": "/opt/silico/bin/silico",
      "args": ["serve", "--workspace", "{folder}"]
    }}
  }}
}}
"#
    )
}

/// Copy MCP config clicked on the start page, and what it copied.
fn copy_config(h: &mut AppHarness) -> Option<String> {
    start_page(h)
        .get_by_role_and_label(Role::Button, "Copy MCP config")
        .click();
    step(h);
    copied(h)
}

#[test]
fn with_no_document_copy_mcp_config_serves_the_newest_recent_ones_folder_then_documents() {
    let f = fixture();
    // The newest listed is gone, with its folder; the next is in a folder of its own.
    let gone = f.dir.path().join("gone").join("a.silico");
    let models = f.dir.path().join("models");
    std::fs::create_dir(&models).unwrap();
    let kept = models.join("b.silico");
    std::fs::write(&kept, EXAMPLE).unwrap();
    std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
    std::fs::write(
        f.recents_file(),
        format!(
            "silico recent-documents 1\n{}\n{}\n",
            gone.to_str().unwrap(),
            kept.to_str().unwrap()
        ),
    )
    .unwrap();
    let documents = f.dir.path().join("Documents");
    let file = f.recents_file();
    let mut h = f.app_made(|app| {
        app.serving_with("/opt/silico/bin/silico".to_owned())
            .documents_folder(Some(documents.clone()))
            .remembering(Recents::at(file))
    });
    step_until(&mut h, "gone marked missing", |h| {
        h.state()
            .recents()
            .is_some_and(|recents| recents.list().first().is_some_and(|recent| recent.missing))
    });
    assert_eq!(
        copy_config(&mut h),
        Some(config_serving(models.to_str().unwrap()))
    );
    assert!(!anywhere(&h, silico_app::start::NO_WORKSPACE_NOTE));
    // With no recent document left that will do: the Documents folder.
    let mut bare = f.app_made(|app| {
        app.serving_with("/opt/silico/bin/silico".to_owned())
            .documents_folder(Some(documents.clone()))
    });
    step(&mut bare);
    assert_eq!(
        copy_config(&mut bare),
        Some(config_serving(documents.to_str().unwrap()))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn with_no_list_kept_recent_says_so() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    assert!(
        start_page(&h)
            .query_by_label(silico_app::start::NOT_KEPT)
            .is_some()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn on_home_a_documents_shortcuts_act_on_nothing_and_esc_and_ctrl_w_return_to_it() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle()
            && h.state().tabs()[0].undoable()
            && anywhere(h, "example.silico *")
    });
    let id = h.state().tabs()[0].id;
    let held = |f: &Fixture| f.sessions.text(&id, None).wait().result.unwrap().text;
    let before = held(&f);
    home_button(&h).click();
    step_until(&mut h, "Home", start_shown);
    // Undo, Redo, Save, Rebuild: none reaches the document hidden.
    for key in [
        eframe::egui::Key::Z,
        eframe::egui::Key::Y,
        eframe::egui::Key::S,
        eframe::egui::Key::B,
    ] {
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, key);
        step(&mut h);
    }
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(start_shown(&h));
    assert_eq!(held(&f), before);
    assert!(h.state().tabs()[0].unsaved());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    // Esc leaves Home for the document; so does Ctrl+W, which closes nothing.
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the document", |h| !start_shown(h));
    home_button(&h).click();
    step_until(&mut h, "Home", start_shown);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::W);
    step_until(&mut h, "the document", |h| !start_shown(h));
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs().len(), 1);
    assert!(!anywhere(
        &h,
        "Save changes to example.silico before closing?"
    ));
    assert_eq!(held(&f), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_document_saved_as_another_file_is_listed_first_in_recent() {
    let f = fixture();
    let old = f.file("example.silico", EXAMPLE);
    let mut h = f.remembering();
    open_and_wait(&mut h, &old);
    let copy = f.dir.path().join("copy.silico");
    save_as(&mut h, &copy);
    step_until(&mut h, "the tab of the new file", |h| {
        h.state().tabs()[0].path == copy.to_str().unwrap()
    });
    home_button(&h).click();
    step_until(&mut h, "Recent, the copy first", |h| {
        start_shown(h) && recent_names(h) == ["copy.silico", "example.silico"]
    });
    assert_eq!(
        std::fs::read_to_string(f.recents_file()).unwrap(),
        format!(
            "silico recent-documents 1\n{}\n{}\n",
            copy.to_str().unwrap(),
            old.to_str().unwrap()
        )
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_warm_start_says_it_is_starting_then_ready() {
    let f = fixture();
    let warm = Warm::new();
    let warming = warm.clone();
    let mut h = f.app_made(|app| app.warming(warming));
    for _ in 0..3 {
        step(&mut h);
    }
    let starting = "Starting the kernel and sketch solver…";
    let ready = "Kernel and sketch solver ready";
    assert!(anywhere(&h, starting));
    assert!(!anywhere(&h, ready));
    // Ready, told off the UI thread (which asks for a frame: start.rs's own test).
    std::thread::spawn(move || warm.ready()).join().unwrap();
    step(&mut h);
    assert!(anywhere(&h, ready));
    assert!(!anywhere(&h, starting));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_warm_start_that_started_nothing_says_the_first_rebuild_starts_them() {
    let f = fixture();
    let warm = Warm::new();
    let mut h = f.app_made(|app| app.warming(warm.clone()));
    step(&mut h);
    warm.cold();
    step(&mut h);
    assert!(anywhere(
        &h,
        "The kernel and sketch solver start with the first rebuild"
    ));
    assert!(!anywhere(&h, "Kernel and sketch solver ready"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_start_page_draws_in_the_themes_colours_on_lavapipe() {
    on_lavapipe(
        "the_start_page_draws_in_the_themes_colours_on_lavapipe",
        || {
            let f = fixture();
            let listed = f.file("bracket.silico", EXAMPLE);
            std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
            std::fs::write(
                f.recents_file(),
                format!(
                    "silico recent-documents 1\n{}\n{}\n",
                    listed.to_str().unwrap(),
                    f.dir.path().join("vise-jaw.silico").to_str().unwrap()
                ),
            )
            .unwrap();
            let state = lavapipe_render_state();
            let present = silico_app::renderer::on_eframe(&state).expect("no renderer");
            let warm = Warm::new();
            let app = App::new(Arc::clone(&f.sessions))
                .presenting(Box::new(present))
                .remembering(Recents::at(f.recents_file()))
                .warming(warm.clone());
            let mut h = {
                let _marked = never_waits::marked();
                Harness::builder()
                    .with_size((1280.0, 800.0))
                    .with_step_dt(1.0 / 60.0)
                    .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                        state.clone(),
                    ))
                    .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
            };
            silico_app::theme::apply(&h.ctx);
            h.set_render_every_step(true);
            recents_read(&mut h);
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let ppp = h.ctx.pixels_per_point();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            // The page's own background, below its columns and above the status bar: the
            // panels' Gray.100.
            let below = eframe::egui::pos2(640.0, 700.0);
            assert_eq!(pixel(below), [0x0d, 0x0d, 0x0d, 0xff]);
            // New part, the primary action: button_primary's Gray.800, inside its fill and
            // short of its icon.
            let new = start_page(&h)
                .get_by_role_and_label(Role::Button, "New part")
                .rect();
            let inside = eframe::egui::pos2(new.left() + 4.0, new.center().y);
            assert_eq!(pixel(inside), [0xe0, 0xdf, 0xdf, 0xff], "{new:?}");
            // A recent document's thumbnail: Gray.0 inside its Gray.250 border.
            let row = start_page(&h)
                .get_by_role_and_label(Role::Button, "bracket.silico")
                .rect();
            let thumb = eframe::egui::pos2(row.left() + 6.0 + 4.0, row.center().y - 12.0);
            assert_eq!(pixel(thumb), [0x00, 0x00, 0x00, 0xff], "{row:?}");
            // Home, on: Gray.250 behind its icon (`.ab-btn.on`), not the selection's blue.
            let home = home_button(&h).rect();
            let edge = eframe::egui::pos2(home.left() + 2.0, home.center().y);
            assert_eq!(pixel(edge), [0x27, 0x26, 0x26, 0xff], "{home:?}");
            assert_eq!(never_waits::waits(), 0);
        },
    );
}

/// The lines of app.rs that asked for the frame before last's next frame (egui's causes,
/// each where `request_repaint` was called).
fn asked_from_app(h: &AppHarness) -> std::collections::BTreeSet<u32> {
    h.ctx
        .repaint_causes()
        .iter()
        .filter(|cause| cause.file.ends_with("src/app.rs"))
        .map(|cause| cause.line)
        .collect()
}

#[test]
fn a_recent_documents_click_that_starts_an_open_asks_for_the_next_frame_itself() {
    // Every session the process may hold already open: the recent document's open is
    // refused at once, as it's asked, so no waker asks for the frame that takes the refusal;
    // the frame whose click started the open asks for it, beyond the idle frame it asks for
    // each time.
    let f = fixture();
    let listed = f.file("listed.silico", EXAMPLE);
    std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
    std::fs::write(
        f.recents_file(),
        format!("silico recent-documents 1\n{}\n", listed.to_str().unwrap()),
    )
    .unwrap();
    for n in 0..silico_core::command::MAX_SESSIONS {
        let path = f.dir.path().join(format!("held{n}.silico"));
        f.sessions.open(&path, true).wait().session.unwrap();
    }
    let mut h = f.remembering();
    step_until(&mut h, "listed", |h| recent_names(h) == ["listed.silico"]);
    quiet_wakes(&mut h);
    idle(&mut h);
    step(&mut h);
    step(&mut h);
    let idle_frame = asked_from_app(&h);
    assert!(!idle_frame.is_empty());
    start_page(&h)
        .get_by_role_and_label(Role::Button, "listed.silico")
        .click();
    let woken = h.state().wakes();
    // The click's frame, the open started and refused; then the next, which takes it.
    step(&mut h);
    step(&mut h);
    assert_eq!(h.state().wakes(), woken);
    let clicked = asked_from_app(&h);
    assert!(
        clicked.iter().any(|line| !idle_frame.contains(line)),
        "{clicked:?} against the idle frame's {idle_frame:?}"
    );
    let said = format!("{} sessions are open", silico_core::command::MAX_SESSIONS);
    step_until(&mut h, "the refusal said", |h| {
        h.state().message().is_some_and(|m| m.contains(&said))
    });
    assert!(h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

// ---- The app bar (docs/design/m5-ui/DESIGN.md, *Layout*; styles.css's `.ab-*`, `.doc-tab`) ----

impl Fixture {
    /// The app drawn in the theme (its fonts and icons, as main.rs applies them), with
    /// `names` open, each the example, the last shown.
    fn themed_with(&self, names: &[&str]) -> AppHarness {
        let mut h = self.app();
        silico_app::theme::apply(&h.ctx);
        for name in names {
            let path = self.file(name, EXAMPLE);
            open_and_wait(&mut h, &path);
        }
        step(&mut h);
        step(&mut h);
        h
    }
}

/// The app bar's button labelled `label` (a tab's, or its close button's).
fn bar_button<'a>(h: &'a AppHarness, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(Role::Button, label)
}

/// Every shape the last frame painted wholly inside `rect`.
fn painted_in(h: &AppHarness, rect: eframe::egui::Rect) -> Vec<eframe::egui::Shape> {
    painted(h)
        .into_iter()
        .filter(|shape| {
            let bounds = shape.visual_bounding_rect();
            bounds.is_positive() && rect.expand(0.5).contains_rect(bounds)
        })
        .collect()
}

/// Whether a tab's close button `label` is drawn: its icon, or anything, inside it.
fn close_drawn(h: &AppHarness, label: &str) -> bool {
    !painted_in(h, bar_button(h, label).rect()).is_empty()
}

/// The texts painted inside `rect`, each with its colour.
fn texts_in(h: &AppHarness, rect: eframe::egui::Rect) -> Vec<(String, eframe::egui::Color32)> {
    painted_in(h, rect)
        .into_iter()
        .filter_map(|shape| match shape {
            eframe::egui::Shape::Text(text) => {
                let color = text
                    .override_text_color
                    .or_else(|| text.galley.job.sections.first().map(|s| s.format.color))
                    .unwrap_or(text.fallback_color);
                Some((text.galley.text().to_owned(), color))
            }
            _ => None,
        })
        .collect()
}

/// The pointer away from the app bar, over the viewport, a frame drawn.
fn pointer_away(h: &mut AppHarness) {
    h.hover_at(eframe::egui::pos2(640.0, 400.0));
    step(h);
    step(h);
}

#[test]
fn the_app_bar_holds_home_file_save_undo_and_redo_then_the_tabs_as_the_prototype_sizes_them() {
    let f = fixture();
    // No document: Home on, File offered, nothing to save, undo, or redo.
    let h = f.themed_with(&[]);
    let buttons: Vec<String> = h
        .query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .take(5)
        .collect();
    assert_eq!(buttons, ["Home", "File", "Save", "Undo", "Redo"]);
    assert_eq!(toggled(&home_button(&h)), Some(true));
    assert!(!bar_button(&h, "File").accesskit_node().is_disabled());
    for off in ["Save", "Undo", "Redo"] {
        assert!(bar_button(&h, off).accesskit_node().is_disabled(), "{off}");
    }
    // Two documents: their tabs after Redo, in the order opened, the last shown.
    let h = f.themed_with(&["example.silico", "other.silico"]);
    let buttons: Vec<String> = h
        .query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .take(9)
        .collect();
    assert_eq!(
        buttons,
        [
            "Home",
            "File",
            "Save",
            "Undo",
            "Redo",
            "example.silico",
            "Close example.silico",
            "other.silico",
            "Close other.silico"
        ]
    );
    assert_eq!(toggled(&home_button(&h)), Some(false));
    assert_eq!(toggled(&bar_button(&h, "other.silico")), Some(true));
    assert_eq!(toggled(&bar_button(&h, "example.silico")), Some(false));
    // `.ab-btn`: 22 high; an icon alone, 14, in 5 of padding at each side; 2 apart
    // (`#appbar`'s `gap`).
    let rect = |label: &str| match label {
        "Home" => home_button(&h).rect(),
        _ => bar_button(&h, label).rect(),
    };
    for icon in ["Home", "Save", "Undo", "Redo"] {
        assert_eq!(rect(icon).size(), eframe::egui::vec2(24.0, 22.0), "{icon}");
    }
    assert_eq!(rect("File").height(), 22.0);
    for (left, right) in [
        ("Home", "File"),
        ("File", "Save"),
        ("Save", "Undo"),
        ("Undo", "Redo"),
    ] {
        assert_eq!(
            rect(right).left() - rect(left).right(),
            2.0,
            "{left} {right}"
        );
    }
    // `.ab-sep`, 1 wide with 6 at each side, 2 from Redo and from the first tab; each tab
    // the bar's 30 less its 1 point border, the buttons centred on it.
    let first = rect("example.silico");
    // egui places widgets on a grid of 1/16 point; File's text width isn't on it.
    let near = |a: f32, b: f32| (a - b).abs() <= 1.0 / 16.0;
    let gap = first.left() - rect("Redo").right();
    assert!(near(gap, 2.0 + 6.0 + 1.0 + 6.0 + 2.0), "{gap}");
    assert_eq!(first.height(), 29.0);
    assert_eq!(rect("Home").center().y, first.center().y);
    // `.x`: 16 square, centred, 6 of padding and the 1 point border after it; the next tab
    // starts there.
    for (tab, close, next) in [
        (
            "example.silico",
            "Close example.silico",
            Some("other.silico"),
        ),
        ("other.silico", "Close other.silico", None),
    ] {
        let x = rect(close);
        assert_eq!(x.size(), eframe::egui::vec2(16.0, 16.0), "{close}");
        assert_eq!(x.center().y, rect(tab).center().y, "{close}");
        if let Some(next) = next {
            assert_eq!(rect(next).left() - x.right(), 6.0 + 1.0, "{close}");
        }
    }
    // `.doc-tab:first-child`'s `border-left`, 1 of Gray.250: the first tab's alone.
    let gray_250 = eframe::egui::Color32::from_rgb(0x27, 0x26, 0x26);
    let left_border = |tab: &str| {
        let x = rect(tab).left() + 0.5;
        painted(&h).iter().any(|shape| {
            matches!(shape, eframe::egui::Shape::LineSegment { points, stroke }
                if *stroke == eframe::egui::Stroke::new(1.0, gray_250)
                    && points.iter().all(|point| (point.x - x).abs() < 1e-3))
        })
    };
    assert!(left_border("example.silico"));
    assert!(!left_border("other.silico"));
    assert_eq!(never_waits::waits(), 0);
}

/// The texts the tooltip titled `title` draws in its title's row, after the title, left to
/// right: its key caps.
fn tooltip_caps(h: &AppHarness, title: &str) -> Vec<String> {
    let row = h.get_by_role_and_label(Role::Label, title).rect();
    let mut caps: Vec<(f32, String)> = painted(h)
        .into_iter()
        .filter_map(|shape| match shape {
            eframe::egui::Shape::Text(text) => {
                let at = eframe::egui::Shape::Text(text.clone()).visual_bounding_rect();
                (at.left() >= row.right() && row.y_range().contains(at.center().y))
                    .then(|| (at.left(), text.galley.text().to_owned()))
            }
            _ => None,
        })
        .collect();
    caps.sort_by(|a, b| a.0.total_cmp(&b.0));
    caps.into_iter().map(|(_, cap)| cap).collect()
}

#[test]
fn save_undo_and_redo_name_their_shortcuts_in_their_tooltips() {
    use eframe::egui::os::OperatingSystem;
    let f = fixture();
    let mut h = f.themed_with(&["example.silico"]);
    // styles.css's `.tooltip`: the title, then the shortcut's key caps, app.js's `CMD`
    // keys as its `KEYNAME` writes them: Mod is ⌘ on macOS and Ctrl elsewhere, Shift ⇧
    // on macOS; Redo is Mod Shift Z on macOS and Mod Y elsewhere.
    for (os, expected) in [
        (
            OperatingSystem::Windows,
            [
                ("Save", vec!["Ctrl", "S"]),
                ("Undo", vec!["Ctrl", "Z"]),
                ("Redo", vec!["Ctrl", "Y"]),
            ],
        ),
        (
            OperatingSystem::Mac,
            [
                ("Save", vec!["⌘", "S"]),
                ("Undo", vec!["⌘", "Z"]),
                ("Redo", vec!["⌘", "⇧", "Z"]),
            ],
        ),
    ] {
        h.ctx.set_os(os);
        for (button, caps) in expected {
            pointer_away(&mut h);
            assert!(h.query_by_role_and_label(Role::Label, button).is_none());
            bar_button(&h, button).hover();
            step_until(&mut h, "its tooltip", |h| {
                h.query_by_role_and_label(Role::Label, button).is_some()
            });
            // Drawn from the frame after it's laid out (egui sizes a new popup unseen).
            step(&mut h);
            assert_eq!(tooltip_caps(&h, button), caps, "{os:?} {button}");
        }
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn tab_first_focuses_home() {
    let f = fixture();
    let mut h = f.themed_with(&["example.silico"]);
    pointer_away(&mut h);
    h.key_press(eframe::egui::Key::Tab);
    step(&mut h);
    step(&mut h);
    assert!(home_button(&h).accesskit_node().is_focused());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn on_home_save_undo_and_redo_are_off_and_on_again_back_on_the_document() {
    let f = fixture();
    let mut h = f.themed_with(&["example.silico"]);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    // Two edits, the second undone: something to save, to undo, and to redo. (Drawn in
    // the theme's fonts, the field's cursor starts at the name's end.)
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the first edit", |h| {
        h.state().tabs()[0].edits.idle() && offered(h, "Undo")
    });
    select(&mut h, "Hole");
    type_name(&mut h, "y");
    step_until(&mut h, "the second edit", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "Holey")
    });
    step_until(&mut h, "Undo offered", |h| offered(h, "Undo"));
    bar_button(&h, "Undo").click();
    step_until(&mut h, "the second undone", |h| {
        shows(h, "Timeline", "Hole") && offered(h, "Redo")
    });
    step_until(&mut h, "all three offered", |h| {
        ["Save", "Undo", "Redo"].iter().all(|word| offered(h, word))
    });
    // Home: the document hidden, none of them acts on it.
    home_button(&h).click();
    step(&mut h);
    step(&mut h);
    assert!(start_shown(&h));
    for word in ["Save", "Undo", "Redo"] {
        assert!(!offered(&h, word), "{word}");
    }
    // Its tab: each offered again.
    bar_button(&h, "example.silico *").click();
    step(&mut h);
    step(&mut h);
    for word in ["Save", "Undo", "Redo"] {
        assert!(offered(&h, word), "{word}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn shift_command_z_redoes_and_command_z_undoes() {
    use eframe::egui::{Key, Modifiers};
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "the edit", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].undoable()
    });
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    step_until(&mut h, "undone", |h| {
        shows(h, "Timeline", "Pin") && h.state().tabs()[0].redoable()
    });
    // DESIGN.md, *Keyboard*: ⇧⌘Z is Redo, beside Ctrl+Y.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    step_until(&mut h, "redone", |h| {
        shows(h, "Timeline", "xPin") && !h.state().tabs()[0].redoable()
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tabs_close_button_is_drawn_while_the_tab_is_hovered_or_shown() {
    let f = fixture();
    let mut h = f.themed_with(&["example.silico", "other.silico"]);
    // At rest: the shown tab's only (`.doc-tab.active .x`).
    pointer_away(&mut h);
    assert!(close_drawn(&h, "Close other.silico"));
    assert!(!close_drawn(&h, "Close example.silico"));
    // Hovered: the other's too (`.doc-tab:hover .x`), in Gray.550 on no fill.
    bar_button(&h, "example.silico").hover();
    step(&mut h);
    step(&mut h);
    assert!(close_drawn(&h, "Close example.silico"));
    assert!(close_drawn(&h, "Close other.silico"));
    let x = bar_button(&h, "Close example.silico").rect();
    let gray_350 = eframe::egui::Color32::from_rgb(0x52, 0x51, 0x51);
    let filled = |h: &AppHarness| {
        painted_in(h, x)
            .iter()
            .any(|shape| matches!(shape, eframe::egui::Shape::Rect(rect) if rect.fill == gray_350))
    };
    assert!(!filled(&h));
    // The close button itself hovered: Gray.350 behind it (`.x:hover`).
    bar_button(&h, "Close example.silico").hover();
    step(&mut h);
    step(&mut h);
    assert!(filled(&h));
    // Home: no tab is shown, so with the pointer away neither is drawn.
    home_button(&h).click();
    step(&mut h);
    pointer_away(&mut h);
    assert!(start_shown(&h));
    assert!(!close_drawn(&h, "Close other.silico"));
    assert!(!close_drawn(&h, "Close example.silico"));
    // The tab focused by the keyboard: its close button drawn, as hovered.
    bar_button(&h, "example.silico").focus();
    step(&mut h);
    step(&mut h);
    assert!(close_drawn(&h, "Close example.silico"));
    assert!(!close_drawn(&h, "Close other.silico"));
    // Undrawn, it's still there and closes its tab, as `.x` at `opacity: 0` does.
    bar_button(&h, "Close example.silico").click_accesskit();
    step_until(&mut h, "the tab closed", |h| h.state().tabs().len() == 1);
    assert_eq!(h.state().tabs()[0].name(), "other.silico");
    assert_eq!(never_waits::waits(), 0);
}

/// A left click at `at`.
fn click_at(h: &mut AppHarness, at: eframe::egui::Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(eframe::egui::Event::PointerButton {
            pos: at,
            button: eframe::egui::PointerButton::Primary,
            pressed,
            modifiers: eframe::egui::Modifiers::NONE,
        });
    }
    step(h);
    step(h);
}

#[test]
fn a_click_beside_a_tabs_close_button_shows_its_document_and_on_it_closes_nothing_else() {
    let f = fixture();
    let mut h = f.themed_with(&["example.silico", "other.silico"]);
    let shown = |h: &AppHarness, name: &str| toggled(&bar_button(h, name)) == Some(true);
    // `.doc-tab` is the whole tab: right of its close button, in its 6 of padding, and
    // above and below the button, each shows the document.
    let x = bar_button(&h, "Close example.silico").rect();
    let tab = bar_button(&h, "example.silico").rect();
    for at in [
        eframe::egui::pos2(x.right() + 3.0, x.center().y),
        eframe::egui::pos2(x.center().x, (tab.top() + x.top()) / 2.0),
        eframe::egui::pos2(x.center().x, (x.bottom() + tab.bottom()) / 2.0),
    ] {
        bar_button(&h, "other.silico").click();
        step_until(&mut h, "other shown", |h| shown(h, "other.silico"));
        click_at(&mut h, at);
        assert!(shown(&h, "example.silico"), "{at:?}");
        assert_eq!(h.state().tabs().len(), 2, "{at:?}");
    }
    // The close button itself closes it.
    click_at(&mut h, x.center());
    step_until(&mut h, "the tab closed", |h| h.state().tabs().len() == 1);
    assert_eq!(h.state().tabs()[0].name(), "other.silico");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_unsaved_documents_tab_ends_in_a_white_asterisk_and_the_shown_ones_name_is_white() {
    let f = fixture();
    let mut h = f.themed_with(&["example.silico", "other.silico"]);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| anywhere(h, "other.silico *"));
    assert!(anywhere(&h, "example.silico"));
    pointer_away(&mut h);
    // `.doc-tab.active`: white; `.doc-tab .dirty`, " *" after the name, white; `.doc-tab`,
    // Gray.550.
    let white = eframe::egui::Color32::from_rgb(0xff, 0xff, 0xff);
    let gray_550 = eframe::egui::Color32::from_rgb(0x93, 0x90, 0x90);
    assert_eq!(
        texts_in(&h, bar_button(&h, "other.silico *").rect()),
        [("other.silico".to_owned(), white), (" *".to_owned(), white)]
    );
    assert_eq!(
        texts_in(&h, bar_button(&h, "example.silico").rect()),
        [("example.silico".to_owned(), gray_550)]
    );
    // Hovered, the other's name is Gray.800 (`.doc-tab:hover`).
    bar_button(&h, "example.silico").hover();
    step(&mut h);
    step(&mut h);
    assert_eq!(
        texts_in(&h, bar_button(&h, "example.silico").rect()),
        [(
            "example.silico".to_owned(),
            eframe::egui::Color32::from_rgb(0xe0, 0xdf, 0xdf)
        )]
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_long_name_ends_in_an_ellipsis_in_a_tab_no_wider_than_220() {
    let f = fixture();
    let long = format!("{}.silico", "a-very-long-document-name".repeat(4));
    let h = f.themed_with(&[long.as_str()]);
    // `.doc-tab`'s `max-width: 220px`, borders and padding in it (`box-sizing:
    // border-box`): from its left to its close button's right, and 6 and 1 after that.
    let tab = bar_button(&h, &long).rect();
    let x = bar_button(&h, &format!("Close {long}")).rect();
    assert_eq!(x.right() + 6.0 + 1.0 - tab.left(), 220.0);
    let names: Vec<_> = painted_in(&h, tab)
        .into_iter()
        .filter_map(|shape| match shape {
            eframe::egui::Shape::Text(text) => Some(text.galley),
            _ => None,
        })
        .collect();
    assert_eq!(names.len(), 1);
    assert!(names[0].elided);
    assert_eq!(names[0].text(), long);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn tabs_wider_than_the_bar_scroll_across_rather_than_squeeze() {
    let f = fixture();
    // Twelve, and a thirteenth later: a process holds sixteen sessions at most.
    let names: Vec<String> = (0..12).map(|n| format!("document-{n:02}.silico")).collect();
    let mut h = f.themed_with(&[]);
    let close = |name: &str| format!("Close {name}");
    // A tab's width: from its left to its close button's, and the 6 of padding and the 1
    // point border after that (`.doc-tab`, `.x`).
    let width = |h: &AppHarness, name: &str| {
        bar_button(h, &close(name)).rect().right() + 6.0 + 1.0 - bar_button(h, name).rect().left()
    };
    // The first four, opened with room to spare: each tab's width then.
    let mut alone = Vec::new();
    for name in &names {
        let path = f.file(name, EXAMPLE);
        open_and_wait(&mut h, &path);
        step(&mut h);
        if alone.len() < 4 {
            alone.push(width(&h, name));
        }
    }
    for _ in 0..5 {
        step(&mut h);
    }
    let redo = bar_button(&h, "Redo").rect();
    let window = h.ctx.content_rect();
    let last = names.last().unwrap();
    // Wider than the window, so scrolled: the newest, shown, in view at the right; the
    // first out of view, left of the separator.
    assert!(width(&h, last) * 12.0 > window.width());
    assert!(
        bar_button(&h, &close(last)).rect().right() <= window.right(),
        "{window:?}"
    );
    assert!(bar_button(&h, &names[0]).rect().left() < redo.right());
    // The wheel over the tabs scrolls them back: the first where it was, after the
    // separator, and the first four as wide as they were.
    let over = bar_button(&h, last).rect().center();
    for _ in 0..10 {
        wheel(&mut h, over, eframe::egui::MouseWheelUnit::Point, 400.0);
    }
    for _ in 0..10 {
        step(&mut h);
    }
    // egui places widgets on a grid of 1/16 point; File's text width isn't on it.
    let gap = bar_button(&h, &names[0]).rect().left() - redo.right();
    assert!(
        (gap - (2.0 + 6.0 + 1.0 + 6.0 + 2.0)).abs() <= 1.0 / 16.0,
        "{gap}"
    );
    let now: Vec<f32> = names[..4].iter().map(|name| width(&h, name)).collect();
    assert_eq!(now, alone);
    assert!(bar_button(&h, last).rect().left() > window.right());
    // A tab shown by an open, past the strip's right as it's scrolled, is scrolled to.
    let newest = f.file("document-12.silico", EXAMPLE);
    open_and_wait(&mut h, &newest);
    for _ in 0..5 {
        step(&mut h);
    }
    let shown = bar_button(&h, "Close document-12.silico").rect();
    assert!(shown.right() <= window.right(), "{shown:?}");
    assert!(shown.left() > redo.right(), "{shown:?}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_app_bar_draws_the_shown_tab_in_gray_200_on_lavapipe() {
    on_lavapipe(
        "the_app_bar_draws_the_shown_tab_in_gray_200_on_lavapipe",
        || {
            let f = fixture();
            meshes_a_real_box(&f);
            let state = lavapipe_render_state();
            let present = silico_app::renderer::on_eframe(&state).expect("no renderer");
            let app = App::new(Arc::clone(&f.sessions)).presenting(Box::new(present));
            let mut h = {
                let _marked = never_waits::marked();
                Harness::builder()
                    .with_size((1280.0, 800.0))
                    .with_step_dt(1.0 / 60.0)
                    .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                        state.clone(),
                    ))
                    .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
            };
            silico_app::theme::apply(&h.ctx);
            h.set_render_every_step(true);
            let path = f.file("bracket.silico", A_BOX);
            h.state_mut().open(path.to_str().unwrap(), false);
            meshes_settled(&mut h);
            let other = f.file("vise-jaw.silico", A_BOX);
            open_and_wait(&mut h, &other);
            bar_button(&h, "bracket.silico").click();
            step_until(&mut h, "bracket shown", |h| {
                toggled(&bar_button(h, "bracket.silico")) == Some(true)
            });
            meshes_settled(&mut h);
            select(&mut h, "Block");
            type_name(&mut h, "x");
            step_until(&mut h, "unsaved", |h| anywhere(h, "bracket.silico *"));
            pointer_away(&mut h);
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let ppp = h.ctx.pixels_per_point();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            let shown = bar_button(&h, "bracket.silico *").rect();
            let other = bar_button(&h, "vise-jaw.silico").rect();
            // The shown tab: Gray.200 (`.doc-tab.active`), in its left padding, clear of its
            // left border and its name; and down over the bar's border below it (`::after`).
            let inside = eframe::egui::pos2(shown.left() + 5.0, shown.center().y);
            assert_eq!(pixel(inside), [0x21, 0x21, 0x21, 0xff], "{shown:?}");
            let below = eframe::egui::pos2(shown.left() + 5.0, shown.bottom() + 0.5);
            assert_eq!(pixel(below), [0x21, 0x21, 0x21, 0xff], "{shown:?}");
            // The other, at rest: the bar's Gray.100, over the bar's Gray.250 border.
            let inside = eframe::egui::pos2(other.left() + 5.0, other.center().y);
            assert_eq!(pixel(inside), [0x0d, 0x0d, 0x0d, 0xff], "{other:?}");
            let below = eframe::egui::pos2(other.left() + 5.0, other.bottom() + 0.5);
            assert_eq!(pixel(below), [0x27, 0x26, 0x26, 0xff], "{other:?}");
            assert_eq!(never_waits::waits(), 0);
        },
    );
}
