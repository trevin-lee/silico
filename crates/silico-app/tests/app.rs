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
    h.get_by_role_and_label(Role::Button, "Redo").click();
    step_until(&mut h, "redone", |h| shows(h, "Timeline", "BAPin"));
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
fn edits_of_two_features_made_while_one_waits_both_land() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Block");
    type_name(&mut h, "x");
    step_until(&mut h, "the transaction begun", |h| {
        h.state().tabs()[0].edits.idle() && h.state().tabs()[0].transaction
    });
    let (hold, check) = hold_queue(&f, &h, 0);
    type_name(&mut h, "y");
    select(&mut h, "Pin");
    type_name(&mut h, "z");
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "both answered", |h| {
        h.state().tabs()[0].edits.idle()
    });
    h.get_by_role_and_label(Role::Button, "Save").click();
    step_until(&mut h, "saved", |h| !anywhere(h, "example.silico *"));
    let names = names_in(&path);
    assert_eq!(names[1], "zPin", "{names:?}");
    assert!(
        names[0].contains('x') && names[0].contains('y'),
        "{names:?}"
    );
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
fn an_emptied_name_is_sent_and_the_cores_answer_said() {
    // The property panel sends every change of its Name, an empty one too (#298's review),
    // through the tab's edits: the core takes it or refuses it, and a refusal is said.
    use silico_app::tab::{Edit, Tab};
    let f = fixture();
    let path = f.file("example.silico", EXAMPLE);
    let opened = f.sessions.open(&path, false).wait();
    let id = opened.session.unwrap().id;
    let mut tab = Tab::opened(
        id,
        path.to_str().unwrap().to_owned(),
        &opened.result.unwrap().problems,
    );
    tab.edit(
        &f.sessions,
        Edit {
            feature: silico_core::document::FeatureId::new("a91cw0de").unwrap(),
            field: "name".to_owned(),
            value: silico_core::document::json::read(b"\"\"").unwrap(),
        },
    );
    let deadline = Instant::now() + WAIT;
    while !(tab.edits.idle() && tab.edits.sent == 1) {
        assert!(Instant::now() < deadline, "never answered");
        tab.poll(&f.sessions);
        std::thread::sleep(Duration::from_millis(2));
    }
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(
        tab.message.is_some() || text.contains(r#""name": """#),
        "neither applied nor said: {text}"
    );
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
