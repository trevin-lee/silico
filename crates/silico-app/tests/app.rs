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
    let h = f.app();
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
    h.get_by_role_and_label(Role::Button, "Open…").click();
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
    h.get_by_role_and_label(Role::Button, "Open…").click();
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
    h.get_by_label("Home").click();
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
    h.get_by_label("Home").click();
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
    // edge, 0.28 of its 96-point side. The front, right, and top face the view, each at
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
    let s = 0.28 * 96.0;
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
    // 220 by 26 points about the canvas's middle, its bottom 8 points above the canvas's:
    // its middle 8 + 13 = 21 points up. Its tools sit in a row along that middle.
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    let canvas = the_viewport(&h);
    let bar = eframe::egui::Rect::from_center_size(
        eframe::egui::pos2(canvas.center().x, canvas.bottom() - 21.0),
        eframe::egui::vec2(220.0, 26.0),
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
    assert!(shows(
        &h,
        "Viewport",
        "The viewport can't draw: no adapter was found"
    ));
    // With a document open too.
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
