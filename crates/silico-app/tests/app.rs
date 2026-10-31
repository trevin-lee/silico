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
use silico_core::cache::Cache;
use silico_core::command::{CheckAnswer, CheckModel, ModelChoice, Running, Sessions};
use silico_core::disk::{Clock, Found, Reader};
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
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
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
    let text = std::fs::read_to_string(path).unwrap();
    text.split(r#""name": ""#)
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap().to_owned())
        .collect()
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
    // Asked to stop a command that has answered since status said it ran: the core's
    // refusal is said in the tab.
    use silico_app::tab::Tab;
    use silico_core::command::{RunningCommand, SessionCommand};
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
    status.running = Some(RunningCommand::Command(SessionCommand::Check));
    tab.status = Some(status);
    assert!(tab.stoppable());
    tab.stop(&f.sessions);
    let said = tab.message.clone().unwrap();
    assert!(said.contains("no check is running in session"), "{said}");
    // The session's own reload running: nothing to stop, the stop off.
    if let Some(status) = &mut tab.status {
        status.running = Some(RunningCommand::Reload);
    }
    assert!(!tab.stoppable());
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
    // "in", "n", then "": the last refused by the gate, its words said in the status bar,
    // the document holding "n".
    assert_eq!(
        h.state().tabs()[0].message.as_deref(),
        Some("name is empty (give the feature a name)")
    );
    assert!(anywhere(&h, "name is empty (give the feature a name)"));
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains(r#""name": "n""#), "{text}");
    assert_eq!(name_field(&h).value().as_deref(), Some(""));
    assert_eq!(never_waits::waits(), 0);
}

/// Pin's Name emptied with Delete (the cursor at its start), each edit answered.
fn empty_name(h: &mut AppHarness) {
    name_field(h).focus();
    step(h);
    for _ in 0.."Pin".len() {
        h.key_press(eframe::egui::Key::Delete);
        step(h);
    }
    step_until(h, "the empty name answered", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].edits.sent == 3
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
