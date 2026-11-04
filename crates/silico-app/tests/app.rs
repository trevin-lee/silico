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
        self.overlaid_at(text, 2.0)
    }

    /// [`overlaid`](Self::overlaid), at `pixels_per_point`.
    fn overlaid_at(&self, text: &str, pixels_per_point: f32) -> (AppHarness, Overlays) {
        let path = self.file("example.silico", text);
        let seen = Overlays::default();
        let app =
            App::new(Arc::clone(&self.sessions)).presenting(Box::new(Marking(Arc::clone(&seen))));
        let mut h = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size((1280.0, 800.0))
                .with_pixels_per_point(pixels_per_point)
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
    h.get_by_role_and_label(Role::Button, item).click();
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

// ---- Sketch mode (M5.11a; section 9, *Sketching*) ----

/// Sketch mode's state, in the first tab.
fn sketch_mode(h: &AppHarness) -> Option<&silico_app::sketch::SketchMode> {
    h.state().tabs()[0].sketch.as_ref()
}

/// Create Sketch from CREATE's menu, on the plane labelled `plane` ("XY plane"): sketch
/// mode open, and its sketch's add answered.
fn create_sketch(h: &mut AppHarness, plane: &str) {
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

/// The sketch bar's Finish Sketch (`true`) or Cancel clicked, and sketch mode closed: once
/// the sketch is read and the bar laid out with its state (it's centred, so it moves as
/// that appears).
fn leave_sketch(h: &mut AppHarness, finish: bool) {
    let word = if finish { "Finish Sketch" } else { "Cancel" };
    step_until(h, "the sketch read", |h| {
        sketch_mode(h).is_some_and(|mode| mode.drawable().is_some())
    });
    let place = |h: &AppHarness| {
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
    settled(h);
}

/// Stepped until sketch mode's newest read holds a sketch.
fn sketch_read(h: &mut AppHarness) {
    step_until(h, "the sketch read", |h| {
        sketch_mode(h).is_some_and(|mode| mode.drawable().is_some())
    });
}

/// `member`, of `kind`, clicked in the canvas, as the viewport's press on it would.
fn select_member(h: &mut AppHarness, member: &str, kind: silico_core::document::ElementKind) {
    let hit = silico_app::sketch::hit::Hit {
        id: silico_core::document::ElementId::new(member).unwrap(),
        kind,
    };
    h.state_mut().tabs_mut()[0]
        .sketch
        .as_mut()
        .unwrap()
        .click(Some(hit), false);
}

fn sketch_id(text: &str) -> silico_core::document::FeatureId {
    silico_core::document::FeatureId::new(text).unwrap()
}

/// An empty sketch named `name` on `plane` (JSON), as the document writes it.
fn empty_sketch(name: &str, plane: &str) -> Vec<(String, Plain)> {
    expected(&format!(
        r#"{{"type": "sketch", "name": "{name}", "plane": {plane}, "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#
    ))
}

#[test]
fn create_sketch_on_xy_adds_an_empty_sketch_opens_sketch_mode_and_finish_keeps_it_one_undo() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create_sketch(&mut h, "XY plane");
    settled(&mut h);
    assert_eq!(
        fields_of(&text_of(&f, &h), "Sketch1"),
        Some(empty_sketch("Sketch1", r#""xy""#))
    );
    // The sketch bar in the canvas, naming it and its plane; the palette's status the
    // empty sketch's; Undo off, nothing of sketch mode's own to undo (never its own add);
    // and the dialogs off.
    step_until(&mut h, "its status", |h| {
        let palette = h.query_by_role_and_label(Role::Group, "Sketch Palette");
        palette.is_some_and(|p| p.query_by_label("Empty").is_some())
            && h.get_by_role_and_label(Role::Toolbar, "Sketch bar")
                .query_by_label("on XY")
                .is_some()
    });
    assert!(
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label("Box")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        h.get_by_role_and_label(Role::Button, "Undo")
            .accesskit_node()
            .is_disabled()
    );
    leave_sketch(&mut h, true);
    assert!(fields_of(&text_of(&f, &h), "Sketch1").is_some());
    assert!(
        h.query_by_role_and_label(Role::Toolbar, "Sketch bar")
            .is_none()
    );
    // One undo takes the whole sketch back.
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step(&mut h);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), EXAMPLE);
    let undos = h.state().tabs()[0]
        .sent
        .iter()
        .filter(|sent| matches!(sent, Sent::Undo(_)))
        .count();
    assert_eq!(undos, 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_of_a_sketch_that_began_the_transaction_rolls_it_back_byte_for_byte() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create_sketch(&mut h, "XZ plane");
    settled(&mut h);
    assert_eq!(
        fields_of(&text_of(&f, &h), "Sketch1"),
        Some(empty_sketch("Sketch1", r#""xz""#))
    );
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), EXAMPLE);
    let tab = &h.state().tabs()[0];
    assert!(!tab.transaction);
    assert_eq!(tab.sent.last(), Some(&Sent::Rollback));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_of_a_sketch_after_earlier_edits_undoes_to_where_it_opened_byte_for_byte() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    settled(&mut h);
    let before = text_of(&f, &h);
    assert!(before.contains(r#""name": "KPin""#));
    let opened_at = h.state().tabs()[0].revision;
    create_sketch(&mut h, "YZ plane");
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sketch_opened_at(), Some(opened_at));
    assert!(fields_of(&text_of(&f, &h), "Sketch1").is_some());
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), before);
    let tab = &h.state().tabs()[0];
    assert!(tab.transaction);
    assert_eq!(tab.sent.last(), Some(&Sent::Undo(Some(opened_at))));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn edit_sketch_from_the_browser_opens_it_at_the_revision_its_asked_at_and_reads_it() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    // An edit first, so the revision it opens at isn't the file's.
    select(&mut h, "Plate");
    type_name(&mut h, "K");
    settled(&mut h);
    let at = h.state().tabs()[0].revision;
    // Its model published first, so sketch mode opens on a model that won't move.
    step_until(&mut h, "the rename's model published", |h| {
        let tab = &h.state().tabs()[0];
        tab.published == Some(tab.revision)
    });
    double_click(&mut h, "Browser", "Triangle");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    let mode = sketch_mode(&h).unwrap();
    assert_eq!(mode.sketch, Some(sketch_id("s0000001")));
    sketch_read(&mut h);
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sketch_opened_at(), Some(at));
    let read = sketch_mode(&h).unwrap().drawable().unwrap();
    let lines: Vec<&str> = read.members.lines.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(lines, ["na000000", "nb000000", "nc000000"]);
    // One read, of the model the rename published, however many frames pass with nothing
    // moving.
    let reads = sketch_mode(&h).unwrap().reads;
    assert_eq!(reads, 1);
    assert_eq!(
        sketch_mode(&h).unwrap().read.as_ref().unwrap().model,
        Some(at)
    );
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(sketch_mode(&h).unwrap().reads, reads);
    // Its own Cancel, nothing of its own sent, undoes nothing.
    let before = text_of(&f, &h);
    let sent = h.state().tabs()[0].sent.len();
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    // And from its context menu.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click_secondary();
    step(&mut h);
    step(&mut h);
    h.get_by_label("Edit Sketch").click();
    step_until(&mut h, "sketch mode again", |h| sketch_mode(h).is_some());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn delete_removes_the_selection_and_the_sketch_loses_those_members() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    // A selection with nothing in it deletes nothing.
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    assert!(h.state().tabs()[0].sent.is_empty());
    select_member(&mut h, "na000000", silico_core::document::ElementKind::Line);
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    let text = text_of(&f, &h);
    // The line goes; both its ends stay, each the end of another side.
    assert!(!text.contains("na000000"), "{text}");
    for kept in ["nb000000", "nc000000", "pa000000", "pb000000", "pc000000"] {
        assert!(text.contains(kept), "{kept}");
    }
    let tab = &h.state().tabs()[0];
    assert!(tab.sent.contains(&Sent::SketchEdit("sketch_remove")));
    assert!(tab.sketch.as_ref().unwrap().selection.is_empty());
    // The next read, of the model the edit published, has two sides.
    step_until(&mut h, "the read after it", |h| {
        sketch_mode(h)
            .and_then(|mode| mode.drawable())
            .is_some_and(|read| read.members.lines.len() == 2)
    });
    assert_eq!(never_waits::waits(), 0);
}

/// A line split at pa000003 into na000001 and na000002, held on one line there by a
/// `tangent_at` (`same`), as `sketch_split` leaves it.
const SPLIT: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Split", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 30, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000003", "construction": false}, {"id": "na000002", "from": "pa000003", "to": "pa000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "tangent_at", "a": "na000001", "b": "na000002", "at": "pa000003", "direction": "same"}]}
  ]
}
"#;

#[test]
fn delete_of_a_split_point_joins_its_pieces() {
    let f = fixture();
    let (mut h, _) = f.opened("split.silico", SPLIT);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Split"));
    double_click(&mut h, "Browser", "Split");
    sketch_read(&mut h);
    select_member(
        &mut h,
        "pa000003",
        silico_core::document::ElementKind::Point,
    );
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    // One line from pa000001 to pa000002 again; the point, the other piece, and the
    // tangency joining them gone (the design's `sketch_join`).
    let joined = r#"{"type": "sketch", "name": "Split", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    assert_eq!(fields_of(&text_of(&f, &h), "Split"), Some(expected(joined)));
    assert!(
        h.state().tabs()[0]
            .sent
            .contains(&Sent::SketchEdit("sketch_join"))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn delete_of_a_corner_no_tangency_holds_says_why_the_join_is_refused() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    // pa000000: na000000 starts there, nc000000 ends there.
    select_member(
        &mut h,
        "pa000000",
        silico_core::document::ElementKind::Point,
    );
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), SKETCHED);
    let tab = &h.state().tabs()[0];
    assert!(tab.sent.contains(&Sent::SketchEdit("sketch_join")));
    let said = tab.message.clone().unwrap_or_default();
    assert!(said.contains("no tangent_at (same)"), "{said}");
    assert!(anywhere(&h, &said));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_in_sketch_mode_clears_the_selection_and_leaves_the_sketch_open() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    select_member(&mut h, "nb000000", silico_core::document::ElementKind::Line);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    let mode = sketch_mode(&h).unwrap();
    assert!(mode.selection.is_empty());
    // The sketch's own selection first; the browser's stays until the next Esc.
    assert_eq!(h.state().tabs()[0].selected, Some(sketch_id("s0000001")));
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(sketch_mode(&h).is_some());
    assert_eq!(h.state().tabs()[0].selected, None);
    assert_eq!(never_waits::waits(), 0);
}

/// Another client's edit, set on `feature`'s name as `name`, at the session's revision now:
/// the revision it makes.
fn other_client_renames(
    f: &Fixture,
    h: &AppHarness,
    feature: &str,
    name: &str,
) -> silico_core::command::Revision {
    let id = h.state().tabs()[0].id;
    let revision = f.sessions.status(&id).session.unwrap().revision;
    let value = json::read(format!("{name:?}").as_bytes()).unwrap();
    let set = f
        .sessions
        .set_field(&id, revision, sketch_id(feature), "name".to_owned(), value)
        .wait();
    assert!(set.result.is_ok(), "{:?}", set.result);
    set.session.unwrap().revision
}

#[test]
fn another_clients_edit_inside_sketch_mode_is_no_reload_and_cancel_undoes_it_with_the_sketchs() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    // An edit before sketch mode opens, which opens the transaction: Cancel keeps it.
    select(&mut h, "Plate");
    type_name(&mut h, "K");
    settled(&mut h);
    let before = text_of(&f, &h);
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    settled(&mut h);
    let opened_at = h.state().tabs()[0].sketch_opened_at().unwrap();
    select_member(&mut h, "na000000", silico_core::document::ElementKind::Line);
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    // Another client's edit inside the transaction: the session never reloads inside one,
    // so it's no reload, and sketch mode still opened where it did.
    let resets = h.state().tabs()[0].resets;
    let moved = other_client_renames(&f, &h, "b0000002", "Spacer");
    step_until(&mut h, "the other client's revision", |h| {
        h.state().tabs()[0].revision == moved
    });
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].resets, resets);
    assert_eq!(h.state().tabs()[0].sketch_opened_at(), Some(opened_at));
    // Cancel undoes back to where it opened: the delete and the other client's edit go,
    // the rename before it stays.
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(
        h.state().tabs()[0].sent.last(),
        Some(&Sent::Undo(Some(opened_at)))
    );
    assert_eq!(never_waits::waits(), 0);
}

/// Four sketches the fake solves as each is made to read (by how many values each sends):
/// one point, under defined; two points and a distance between them, its constraint in
/// conflict, over defined; three points, no solution; four points, solved as sent, fully
/// defined.
const FOUR_SKETCHES: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Loose", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []},
    {"id": "s0000002", "type": "sketch", "v": 1, "name": "Tight", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 5, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "distance", "a": "pa000001", "b": "pa000002", "value": 5, "driving": true}]},
    {"id": "s0000003", "type": "sketch", "v": 1, "name": "Stuck", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 5, "y": 0, "construction": false}, {"id": "pa000003", "x": 0, "y": 5, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []},
    {"id": "s0000004", "type": "sketch", "v": 1, "name": "Fixed", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 5, "y": 0, "construction": false}, {"id": "pa000003", "x": 0, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

#[test]
fn each_sketch_is_read_for_its_browser_prefix_by_its_state_as_solidworks_prefixes_it() {
    use silico_protocol::{SolveStatus, Solved};
    let f = fixture();
    // The values each sends: the origin and the axes' ends (6), each point's two, and the
    // distance's value.
    f.sketches.answer(|system| {
        let sent = FakeSketches::as_sent(system);
        Ok(match system.values.len() {
            8 => Solved {
                dofs: 2,
                free: system.unknowns.clone(),
                ..sent
            },
            11 => Solved {
                conflicting: system.constraints.iter().filter_map(|c| c.tag()).collect(),
                ..sent
            },
            12 => Solved {
                status: SolveStatus::Failed,
                algorithm: None,
                ..sent
            },
            _ => sent,
        })
    });
    let (mut h, _) = f.opened("four.silico", FOUR_SKETCHES);
    // What the browser's rows are prefixed by (drawn by the browser's port).
    let prefixes = |h: &AppHarness| {
        ["s0000001", "s0000002", "s0000003", "s0000004"]
            .map(|id| h.state().tabs()[0].prefixes.prefix(&sketch_id(id)))
    };
    step_until(&mut h, "the prefixes", |h| {
        h.state().tabs()[0]
            .prefixes
            .state(&sketch_id("s0000004"))
            .is_some()
            && prefixes(h) == [Some("(–)"), Some("(+)"), Some("(?)"), None]
    });
    // Each sketch read once for the model, however many frames pass.
    let reads = h.state().tabs()[0].prefixes.reads;
    assert_eq!(reads, 4);
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].prefixes.reads, reads);
    assert_eq!(never_waits::waits(), 0);
}

/// A box, and a sketch on its top face: a face the fake doesn't describe as a plane, so the
/// sketch's plane is broken.
const ON_A_FACE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10},
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Top", "plane": {"face": "b0000001.box.Face6", "x": "+x", "reversed": false}, "points": [{"id": "pa000001", "x": 1, "y": 1, "construction": false}, {"id": "pa000002", "x": 5, "y": 1, "construction": false}, {"id": "pa000003", "x": 5, "y": 3, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

#[test]
fn a_face_sketch_whose_face_broke_opens_says_its_plane_problem_and_draws_flat() {
    let f = fixture();
    let (mut h, path) = f.opened("face.silico", ON_A_FACE);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Top"));
    // What the core's own read says of the plane, as an agent reads it.
    let id = h.state().tabs()[0].id;
    let core = f
        .sessions
        .sketch(&id, ModelChoice::Current, sketch_id("s0000001"))
        .wait()
        .result
        .unwrap();
    let problem = core.plane.clone().unwrap_err();
    double_click(&mut h, "Browser", "Top");
    sketch_read(&mut h);
    let want = format!("The sketch's plane: {}", problem.message());
    step_until(&mut h, "its plane's problem", |h| anywhere(h, &want));
    // Its suggestion too (said in Properties as well, as the sketch's mark's).
    if let Some(suggestion) = problem.suggestion() {
        assert!(h.query_all_by_label(suggestion).next().is_some());
    }
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    assert_eq!(read.plane, core.plane);
    // Drawn flat in a canvas's place, every member within it.
    let rect = eframe::egui::Rect::from_min_size(
        eframe::egui::pos2(300.0, 100.0),
        eframe::egui::vec2(600.0, 400.0),
    );
    let flat = silico_app::sketch::draw::Flat::fit(rect, &read);
    let project = |at| flat.project(at);
    let view = silico_app::sketch::draw::View {
        project: &project,
        scale: flat.scale(),
        visible: None,
        chord_tolerance: silico_app::sketch::style::SKETCH_CHORD_TOLERANCE,
    };
    let drawn = silico_app::sketch::draw::drawing(
        &read,
        &view,
        silico_app::sketch::draw::Shown::default(),
        &Default::default(),
        &silico_app::sketch::draw::region_fills(
            &read,
            flat.scale(),
            silico_app::sketch::style::SKETCH_CHORD_TOLERANCE,
        ),
    );
    // The origin (within the fit too), the triangle's fill (its one region), three sides,
    // and three corners.
    assert_eq!(drawn.len(), 8);
    assert!(
        drawn
            .iter()
            .any(|d| matches!(d, silico_app::sketch::draw::Drawn::Fill { .. }))
    );
    for item in &drawn {
        assert!(
            item.positions().iter().all(|p| rect.contains(*p)),
            "{item:?}"
        );
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), ON_A_FACE);
    assert_eq!(never_waits::waits(), 0);
}

/// The fake, each box's faces described as OCCT's planes (docs/ports/attacher.md, *Worked
/// frames*: Face1 −X, Face2 +X, Face3 −Y, Face4 +Y, Face5 −Z, Face6 +Z), through the origin.
fn planar_boxes(kernel: &FakeKernel) {
    use silico_protocol::{Reply, Surface};
    const NORMALS: [[f64; 3]; 6] = [
        [-1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
    ];
    kernel.answer_instead(|op| {
        let Op::DescribeAll { shape } = op else {
            return None;
        };
        if !FakeKernel::describe(shape)?.starts_with("box") {
            return None;
        }
        let mut description = FakeKernel::described(shape)?;
        for (face, normal) in description.faces.iter_mut().zip(NORMALS) {
            face.surface = Surface::Plane {
                normal,
                location: [0.0; 3],
            };
        }
        Some(Reply::DescribedAll(description))
    });
}

/// A box alone.
const BLOCK: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}
  ]
}
"#;

#[test]
fn create_sketch_on_a_face_writes_the_face_and_the_core_fills_in_its_x() {
    let f = fixture();
    planar_boxes(&f.kernel);
    let (mut h, _) = f.opened("block.silico", BLOCK);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let sessions = Arc::clone(h.state().sessions());
    h.state_mut().tabs_mut()[0].create_sketch_on_face(&sessions, "b0000001.box.Face6");
    step_until(&mut h, "its sketch added", |h| {
        sketch_mode(h).is_some_and(|mode| mode.sketch.is_some())
    });
    settled(&mut h);
    // Face6 faces +Z: its x is +x, the attacher's default for a normal along z.
    assert_eq!(
        fields_of(&text_of(&f, &h), "Sketch1"),
        Some(empty_sketch(
            "Sketch1",
            r#"{"face": "b0000001.box.Face6", "x": "+x", "reversed": false}"#
        ))
    );
    sketch_read(&mut h);
    assert!(sketch_mode(&h).unwrap().plane_problem().is_none());
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), BLOCK);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn create_sketch_on_a_face_the_core_refuses_says_why_and_leaves_sketch_mode_and_no_transaction() {
    // The fake's own faces aren't planes: the add is refused.
    let f = fixture();
    let (mut h, _) = f.opened("block.silico", BLOCK);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let sessions = Arc::clone(h.state().sessions());
    h.state_mut().tabs_mut()[0].create_sketch_on_face(&sessions, "b0000001.box.Face6");
    assert!(sketch_mode(&h).is_some());
    step_until(&mut h, "sketch mode left", |h| sketch_mode(h).is_none());
    settled(&mut h);
    let tab = &h.state().tabs()[0];
    assert!(tab.message.is_some());
    assert!(!tab.transaction);
    assert_eq!(text_of(&f, &h), BLOCK);
    // Hostile names: quotes and backslashes reach the core as the string they are.
    let sessions = Arc::clone(h.state().sessions());
    h.state_mut().tabs_mut()[0].create_sketch_on_face(&sessions, "a\"b\\c");
    step_until(&mut h, "sketch mode left again", |h| {
        sketch_mode(h).is_none()
    });
    settled(&mut h);
    assert!(h.state().tabs()[0].message.is_some());
    assert_eq!(text_of(&f, &h), BLOCK);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_refused_read_is_kept_and_said_and_delete_still_goes_to_the_core() {
    // The sketch suppressed by another client after sketch mode opened on it: its read is
    // refused, kept, and said. A suppressed sketch can still be edited (section 7), so
    // Delete still sends its removal, for the core to judge.
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    let id = h.state().tabs()[0].id;
    let revision = h.state().tabs()[0].revision;
    f.sessions
        .suppress_feature(&id, revision, sketch_id("s0000001"), true)
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the refusal", |h| {
        sketch_mode(h)
            .and_then(|mode| mode.read.as_ref())
            .is_some_and(|read| read.result.is_err())
    });
    let said = {
        let mode = sketch_mode(&h).unwrap();
        mode.palette_view(h.state().tabs()[0].revision, None).said
    };
    let said = said.unwrap();
    step_until(&mut h, "it said", |h| anywhere(h, &said));
    select_member(&mut h, "na000000", silico_core::document::ElementKind::Line);
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    assert_eq!(
        h.state().tabs()[0].sent.last(),
        Some(&Sent::SketchEdit("sketch_remove"))
    );
    assert!(!text_of(&f, &h).contains("na000000"));
    assert_eq!(never_waits::waits(), 0);
}

/// `member`, a line, selected and deleted in the open sketch, and everything answered.
fn delete_line(h: &mut AppHarness, member: &str) {
    select_member(h, member, silico_core::document::ElementKind::Line);
    h.key_press(eframe::egui::Key::Delete);
    step(h);
    settled(h);
}

/// SKETCHED open, with Plate renamed KPlate first if `edited`; then Triangle opened in
/// sketch mode and read. The text before sketch mode opened, and the revision it opened at.
fn triangle_open(f: &Fixture, edited: bool) -> (AppHarness, String) {
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    if edited {
        select(&mut h, "Plate");
        type_name(&mut h, "K");
        settled(&mut h);
    }
    let before = text_of(f, &h);
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    settled(&mut h);
    (h, before)
}

#[test]
fn finish_after_two_deletes_inside_a_transaction_is_one_undo_step_back_to_before_it_opened() {
    let f = fixture();
    let (mut h, before) = triangle_open(&f, true);
    assert!(before.contains(r#""name": "KPlate""#));
    delete_line(&mut h, "na000000");
    delete_line(&mut h, "nb000000");
    let text = text_of(&f, &h);
    assert!(!text.contains("na000000") && !text.contains("nb000000"));
    leave_sketch(&mut h, true);
    let undos = |h: &AppHarness| {
        h.state().tabs()[0]
            .sent
            .iter()
            .filter(|sent| matches!(sent, Sent::Undo(_)))
            .count()
    };
    assert_eq!(undos(&h), 0);
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step(&mut h);
    settled(&mut h);
    // One undo, of both deletes at once: the text before sketch mode opened, the rename kept.
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(undos(&h), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_after_a_delete_that_began_the_transaction_rolls_it_back_and_leaves_none_open() {
    let f = fixture();
    let (mut h, before) = triangle_open(&f, false);
    assert_eq!(before, SKETCHED);
    delete_line(&mut h, "na000000");
    assert!(!text_of(&f, &h).contains("na000000"));
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), SKETCHED);
    let tab = &h.state().tabs()[0];
    assert!(!tab.transaction);
    assert_eq!(tab.sent.last(), Some(&Sent::Rollback));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_after_a_delete_following_an_earlier_edit_undoes_to_where_it_opened() {
    let f = fixture();
    let (mut h, before) = triangle_open(&f, true);
    let opened_at = h.state().tabs()[0].sketch_opened_at().unwrap();
    delete_line(&mut h, "na000000");
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), before);
    let tab = &h.state().tabs()[0];
    assert!(tab.transaction);
    assert_eq!(tab.sent.last(), Some(&Sent::Undo(Some(opened_at))));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn undo_and_redo_step_inside_sketch_mode_never_past_where_it_opened() {
    use eframe::egui::{Key, Modifiers};
    let f = fixture();
    let (mut h, before) = triangle_open(&f, true);
    let undo_on = |h: &AppHarness| h.state().tabs()[0].undoable();
    // Nothing of sketch mode's to undo yet, though the rename is: off.
    assert!(!undo_on(&h));
    delete_line(&mut h, "na000000");
    delete_line(&mut h, "nb000000");
    let lines = |f: &Fixture, h: &AppHarness| {
        let text = text_of(f, h);
        ["na000000", "nb000000", "nc000000"]
            .into_iter()
            .filter(|line| text.contains(line))
            .collect::<Vec<_>>()
    };
    assert_eq!(lines(&f, &h), ["nc000000"]);
    let press = |h: &mut AppHarness, key| {
        // A frame first, so the status read is of the revision the last step came to.
        step(h);
        h.key_press_modifiers(Modifiers::COMMAND, key);
        step(h);
        settled(h);
    };
    // Ctrl+Z: the second delete back.
    press(&mut h, Key::Z);
    assert_eq!(lines(&f, &h), ["nb000000", "nc000000"]);
    // Ctrl+Y: gone again.
    press(&mut h, Key::Y);
    assert_eq!(lines(&f, &h), ["nc000000"]);
    // Twice more: both back, at where it opened; a third does nothing.
    press(&mut h, Key::Z);
    press(&mut h, Key::Z);
    assert_eq!(text_of(&f, &h), before);
    assert!(sketch_mode(&h).is_some());
    let sent = h.state().tabs()[0].sent.len();
    press(&mut h, Key::Z);
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    assert_eq!(text_of(&f, &h), before);
    assert!(!undo_on(&h));
    // Redo gives back only what it undid: twice, then nothing.
    press(&mut h, Key::Y);
    press(&mut h, Key::Y);
    assert_eq!(lines(&f, &h), ["nc000000"]);
    assert!(!h.state().tabs()[0].redoable());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn after_an_edit_each_sketch_is_read_once_more_and_a_changed_state_shows_its_prefix() {
    use silico_protocol::{SolveStatus, Solved};
    let f = fixture();
    f.sketches.answer(|system| {
        let sent = FakeSketches::as_sent(system);
        Ok(match system.values.len() {
            8 => Solved {
                dofs: 2,
                free: system.unknowns.clone(),
                ..sent
            },
            11 => Solved {
                conflicting: system.constraints.iter().filter_map(|c| c.tag()).collect(),
                ..sent
            },
            12 => Solved {
                status: SolveStatus::Failed,
                algorithm: None,
                ..sent
            },
            _ => sent,
        })
    });
    let (mut h, _) = f.opened("four.silico", FOUR_SKETCHES);
    let prefixes = |h: &AppHarness| {
        ["s0000001", "s0000002", "s0000003", "s0000004"]
            .map(|id| h.state().tabs()[0].prefixes.prefix(&sketch_id(id)))
    };
    step_until(&mut h, "the prefixes", |h| {
        prefixes(h) == [Some("(–)"), Some("(+)"), Some("(?)"), None]
    });
    assert_eq!(h.state().tabs()[0].prefixes.reads, 4);
    // Loose's one point deleted, in sketch mode: empty, it's fully defined. Its prefix
    // comes from sketch mode's own read; the other three are read once more, of the model
    // the edit published.
    double_click(&mut h, "Browser", "Loose");
    sketch_read(&mut h);
    select_member(
        &mut h,
        "pa000001",
        silico_core::document::ElementKind::Point,
    );
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    settled(&mut h);
    step_until(&mut h, "the prefixes again", |h| {
        h.state().tabs()[0].prefixes.reads == 7
            && prefixes(h) == [None, Some("(+)"), Some("(?)"), None]
    });
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].prefixes.reads, 7);
    assert_eq!(never_waits::waits(), 0);
}

/// Ctrl (⌘) and `key` pressed, a frame after the last answer so the status read is of the
/// revision it came to, and everything it sent answered.
fn chord(h: &mut AppHarness, key: eframe::egui::Key) {
    step(h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, key);
    step(h);
    settled(h);
}

fn undos(h: &AppHarness) -> usize {
    h.state().tabs()[0]
        .sent
        .iter()
        .filter(|sent| matches!(sent, Sent::Undo(_)))
        .count()
}

/// SKETCHED with a second sketch, Bar: one line from (0, 30) to (20, 30).
fn two_sketches() -> String {
    SKETCHED.replace(
        "  ]\n}",
        r#"  ,
    {"id": "s0000002", "type": "sketch", "v": 1, "name": "Bar", "plane": "xy", "points": [{"id": "pd000000", "x": 0, "y": 30, "construction": false}, {"id": "pe000000", "x": 20, "y": 30, "construction": false}], "lines": [{"id": "nd000000", "from": "pd000000", "to": "pe000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}
  ]
}"#,
    )
}

#[test]
fn a_sketch_opened_while_the_last_ones_finish_waits_and_its_undo_never_passes_where_it_opened() {
    let f = fixture();
    let text = two_sketches();
    let (mut h, _) = f.opened("two.silico", &text);
    step_until(&mut h, "its sketches", |h| shows(h, "Browser", "Bar"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    delete_line(&mut h, "na000000");
    // The second delete sent, and before it answers, Finish, then Edit Sketch on Bar: the
    // Finish is still to be taken, so Bar doesn't open yet.
    let sessions = Arc::clone(h.state().sessions());
    select_member(&mut h, "nb000000", silico_core::document::ElementKind::Line);
    let tab = &mut h.state_mut().tabs_mut()[0];
    tab.sketch_delete(&sessions);
    tab.finish_sketch(&sessions, true);
    assert!(!tab.can_sketch());
    tab.edit_sketch(&sessions, &sketch_id("s0000002"));
    assert!(tab.sketch.is_none());
    settled(&mut h);
    // Now it opens, after both deletes and the Finish.
    double_click(&mut h, "Browser", "Bar");
    sketch_read(&mut h);
    settled(&mut h);
    let opened = text_of(&f, &h);
    assert!(!opened.contains("na000000") && !opened.contains("nb000000"));
    // Its undo is off: nothing of its own to undo, the Triangle's Finish behind it.
    let sent = undos(&h);
    chord(&mut h, eframe::egui::Key::Z);
    assert_eq!(undos(&h), sent);
    assert_eq!(text_of(&f, &h), opened);
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), opened);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_second_undo_queued_inside_sketch_mode_is_dropped_at_where_it_opened() {
    let f = fixture();
    let (mut h, before) = triangle_open(&f, true);
    delete_line(&mut h, "na000000");
    // Two undos asked before either answers (as a client could): the first brings it back
    // to where it opened; the second, come up there, is dropped, never undoing the rename.
    let sessions = Arc::clone(h.state().sessions());
    let tab = &mut h.state_mut().tabs_mut()[0];
    tab.step(&sessions, true);
    tab.step(&sessions, true);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(undos(&h), 1);
    // The one undo that ran is the one counted: one for Redo to give back.
    assert_eq!(sketch_mode(&h).unwrap().undone, 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn finish_after_an_undo_inside_keeps_what_was_undone_undone() {
    let f = fixture();
    let (mut h, before) = triangle_open(&f, true);
    delete_line(&mut h, "na000000");
    delete_line(&mut h, "nb000000");
    chord(&mut h, eframe::egui::Key::Z);
    let finished = text_of(&f, &h);
    assert!(finished.contains("nb000000") && !finished.contains("na000000"));
    leave_sketch(&mut h, true);
    // Redo off at Finish's end: the undone delete isn't redone past it.
    assert!(!h.state().tabs()[0].redoable());
    let sent = h.state().tabs()[0].sent.len();
    chord(&mut h, eframe::egui::Key::Y);
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    assert_eq!(text_of(&f, &h), finished);
    // One Undo takes sketch mode's edits back, the rename kept; Redo gives them back.
    chord(&mut h, eframe::egui::Key::Z);
    assert_eq!(text_of(&f, &h), before);
    chord(&mut h, eframe::egui::Key::Y);
    assert_eq!(text_of(&f, &h), finished);
    assert!(!h.state().tabs()[0].redoable());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn finish_undone_back_to_where_a_sketch_began_the_transaction_leaves_none_open() {
    let f = fixture();
    let (mut h, before) = triangle_open(&f, false);
    delete_line(&mut h, "na000000");
    chord(&mut h, eframe::egui::Key::Z);
    assert_eq!(text_of(&f, &h), before);
    leave_sketch(&mut h, true);
    let tab = &h.state().tabs()[0];
    assert!(!tab.transaction);
    assert_eq!(tab.sent.last(), Some(&Sent::Rollback));
    assert_eq!(text_of(&f, &h), SKETCHED);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn redo_inside_sketch_mode_never_gives_back_what_was_undone_before_it_opened() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    select(&mut h, "Plate");
    type_name(&mut h, "K");
    settled(&mut h);
    chord(&mut h, eframe::egui::Key::Z);
    assert_eq!(text_of(&f, &h), SKETCHED);
    assert!(h.state().tabs()[0].redoable());
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    settled(&mut h);
    assert!(!h.state().tabs()[0].redoable());
    let sent = h.state().tabs()[0].sent.len();
    chord(&mut h, eframe::egui::Key::Y);
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    assert_eq!(text_of(&f, &h), SKETCHED);
    // Finished, having changed nothing: the rename's redo is the person's again.
    leave_sketch(&mut h, true);
    assert!(h.state().tabs()[0].redoable());
    chord(&mut h, eframe::egui::Key::Y);
    assert!(text_of(&f, &h).contains(r#""name": "KPlate""#));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_edit_after_a_finish_that_fenced_redo_lets_its_own_undo_be_redone() {
    let f = fixture();
    let (mut h, _) = triangle_open(&f, true);
    delete_line(&mut h, "na000000");
    delete_line(&mut h, "nb000000");
    chord(&mut h, eframe::egui::Key::Z);
    leave_sketch(&mut h, true);
    assert!(!h.state().tabs()[0].redoable());
    // An edit, then its Undo: the redo is the edit's, at Finish's end state once more, and
    // the fence went with the edit.
    select(&mut h, "KPlate");
    type_name(&mut h, "Q");
    settled(&mut h);
    chord(&mut h, eframe::egui::Key::Z);
    assert!(h.state().tabs()[0].redoable());
    chord(&mut h, eframe::egui::Key::Y);
    assert!(text_of(&f, &h).contains(r#""name": "QKPlate""#));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn what_cancel_discarded_isnt_redoable_after_it() {
    let f = fixture();
    let (mut h, before) = triangle_open(&f, true);
    delete_line(&mut h, "na000000");
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), before);
    // Cancel undid the delete; Redo would give it back, so it's off, and Ctrl+Y sends
    // nothing.
    assert!(!h.state().tabs()[0].redoable());
    let sent = h.state().tabs()[0].sent.len();
    chord(&mut h, eframe::egui::Key::Y);
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn finish_or_cancel_back_at_where_it_opened_by_an_undo_changes_nothing_of_the_undo_stack() {
    for finish in [true, false] {
        let f = fixture();
        let (mut h, before) = triangle_open(&f, true);
        delete_line(&mut h, "na000000");
        chord(&mut h, eframe::egui::Key::Z);
        assert_eq!(text_of(&f, &h), before);
        let sent = undos(&h);
        leave_sketch(&mut h, finish);
        // Nothing grouped, nothing undone: the state is where it opened.
        assert_eq!(undos(&h), sent, "finish: {finish}");
        assert_eq!(text_of(&f, &h), before);
        // The next Undo is the rename's.
        chord(&mut h, eframe::egui::Key::Z);
        assert_eq!(text_of(&f, &h), SKETCHED, "finish: {finish}");
        assert_eq!(never_waits::waits(), 0);
    }
}

// ---- Sketch tools (M5.11b; section 9, *Sketching*) ----

/// The projection the tools are driven through by hand ([`sketch_events`]), which the pane's
/// own overlay is made to match ([`hand_camera`]): ten points a millimetre, the origin at
/// (100, 500), y up.
fn to_screen(at: [f64; 2]) -> Option<eframe::egui::Pos2> {
    Some(eframe::egui::pos2(
        (100.0 + 10.0 * at[0]) as f32,
        (500.0 - 10.0 * at[1]) as f32,
    ))
}

/// [`to_screen`]'s way back.
fn to_sketch(at: eframe::egui::Pos2) -> Option<[f64; 2]> {
    Some([
        (f64::from(at.x) - 100.0) / 10.0,
        (500.0 - f64::from(at.y)) / 10.0,
    ])
}

fn pointer(
    action: silico_app::sketch::PointerAction,
    x: f64,
    y: f64,
) -> silico_app::sketch::PointerEvent {
    silico_app::sketch::PointerEvent {
        action,
        at: to_screen([x, y]).unwrap(),
        shift: false,
        ctrl: false,
    }
}

fn clicked(x: f64, y: f64) -> Vec<silico_app::sketch::PointerEvent> {
    use silico_app::sketch::PointerAction::{Press, Release};
    vec![pointer(Press, x, y), pointer(Release, x, y)]
}

fn dragged(from: [f64; 2], to: [f64; 2]) -> Vec<silico_app::sketch::PointerEvent> {
    use silico_app::sketch::PointerAction::{Move, Press, Release};
    vec![
        pointer(Press, from[0], from[1]),
        pointer(Move, (from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0),
        pointer(Move, to[0], to[1]),
        pointer(Release, to[0], to[1]),
    ]
}

/// `events` given to the first tab's sketch mode as one frame of the viewport's overlay
/// gives them (`Tab::sketch_frame`, which the pane's `Tab::overlay` calls), through
/// [`to_screen`], on a thread marked so a wait there counts, the pointer at the last; then
/// one frame of the app's, whose pane draws through the same projection ([`hand_camera`]).
fn sketch_events(f: &Fixture, h: &mut AppHarness, events: &[silico_app::sketch::PointerEvent]) {
    hand_camera(h);
    {
        let _marked = never_waits::marked();
        let ctx = eframe::egui::Context::default();
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        let pointer = events.last().map(|event| event.at);
        let mut output = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            let painter = ui.painter().clone();
            let input = silico_app::sketch::FrameInput {
                events,
                pointer,
                project: &to_screen,
                to_plane: &to_sketch,
            };
            tab.sketch_frame(&sessions, &painter, &input);
        });
        output.textures_delta.clear();
    }
    step(h);
}

/// The viewport's camera set so the pane's own overlay, which draws sketch mode every frame
/// through the renderer's projection, draws XY as [`to_screen`] does: orthographic, straight
/// down, a tenth of a millimetre a point, the image's centre over the sketch's place
/// `to_sketch` gives it. Nothing until the pane has drawn a frame.
fn hand_camera(h: &mut AppHarness) {
    let Some(frame) = view(h).frame() else {
        return;
    };
    let ppp = f64::from(frame.pixels_per_point);
    let [width, height] = frame.size_px.map(f64::from);
    // The image's centre on the screen, in f64, and where `to_sketch` puts it.
    let centre = [
        f64::from(frame.rect.left()) + width / 2.0 / ppp,
        f64::from(frame.rect.top()) + height / 2.0 / ppp,
    ];
    let [x, y] = [(centre[0] - 100.0) / 10.0, (500.0 - centre[1]) / 10.0];
    view_mut(h).set_camera(silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic {
            view_height: height / ppp / 10.0,
        },
        eye: [x, y, 1000.0],
        target: [x, y, 0.0],
        up: [0.0, 1.0, 0.0],
    });
}

/// The sketch `id` as the session's document holds it.
fn sketch_of_text(f: &Fixture, h: &AppHarness, id: &str) -> silico_core::document::Sketch {
    let document = silico_core::document::read(text_of(f, h).as_bytes()).unwrap();
    document
        .features
        .into_iter()
        .find_map(|feature| match feature.operation {
            silico_core::document::Operation::Sketch(sketch) if feature.id.as_str() == id => {
                Some(sketch)
            }
            _ => None,
        })
        .unwrap()
}

/// The ID of the point of `sketch` at (x, y), exactly.
fn point_at(sketch: &silico_core::document::Sketch, x: f64, y: f64) -> String {
    let found: Vec<&str> = sketch
        .points
        .iter()
        .filter(|p| p.x.mm() == x && p.y.mm() == y)
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(found.len(), 1, "one point at ({x}, {y}) in {sketch:?}");
    found[0].to_owned()
}

/// The line of `sketch` from the point `a` to `b`, either way, if there's one.
fn line_between(sketch: &silico_core::document::Sketch, a: &str, b: &str) -> Option<String> {
    sketch
        .lines
        .iter()
        .find(|l| {
            (l.from.as_str() == a && l.to.as_str() == b)
                || (l.from.as_str() == b && l.to.as_str() == a)
        })
        .map(|l| l.id.as_str().to_owned())
}

/// Whether `sketch` holds a `horizontal` (or `vertical`) of the line `line`.
fn holds_direction(sketch: &silico_core::document::Sketch, line: &str, horizontal: bool) -> bool {
    use silico_core::document::{Form, Reference};
    sketch
        .constraints
        .iter()
        .any(|c| match (&c.form, horizontal) {
            (
                Form::Horizontal {
                    line: Reference::Element(l),
                },
                true,
            )
            | (
                Form::Vertical {
                    line: Reference::Element(l),
                },
                false,
            ) => l.as_str() == line,
            _ => false,
        })
}

/// `sketch` holds the rectangle from `lo` to `hi`, worked by hand: a point at each corner,
/// four lines from corner to corner sharing them, the bottom and top `horizontal`, and the
/// sides `vertical`.
fn assert_rectangle_in(sketch: &silico_core::document::Sketch, lo: [f64; 2], hi: [f64; 2]) {
    let a = point_at(sketch, lo[0], lo[1]);
    let b = point_at(sketch, hi[0], lo[1]);
    let c = point_at(sketch, hi[0], hi[1]);
    let d = point_at(sketch, lo[0], hi[1]);
    for (from, to, horizontal) in [
        (&a, &b, true),
        (&b, &c, false),
        (&c, &d, true),
        (&d, &a, false),
    ] {
        let side = line_between(sketch, from, to).unwrap_or_else(|| panic!("{from} to {to}"));
        assert!(holds_direction(sketch, &side, horizontal), "{side}");
        assert!(!holds_direction(sketch, &side, !horizontal), "{side}");
    }
}

fn adds_sent(h: &AppHarness) -> usize {
    h.state().tabs()[0]
        .sent
        .iter()
        .filter(|sent| **sent == Sent::SketchEdit("sketch_add"))
        .count()
}

/// Stepped until sketch mode's newest read has `lines` lines.
fn read_with_lines(h: &mut AppHarness, lines: usize) {
    step_until(h, "the read after the edit", |h| {
        sketch_mode(h)
            .and_then(|mode| mode.drawable())
            .is_some_and(|read| read.members.lines.len() == lines)
    });
}

#[test]
fn a_corner_rectangle_dragged_and_one_by_two_clicks_each_add_four_lines_on_four_shared_points() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create_sketch(&mut h, "XY plane");
    sketch_read(&mut h);
    let id = sketch_mode(&h).unwrap().sketch.clone().unwrap();
    // R chooses the corner rectangle (DESIGN.md, *Keyboard*); the palette names it.
    h.key_press(eframe::egui::Key::R);
    step(&mut h);
    use silico_app::sketch::tools::Choice;
    use silico_core::sketch_tools::ToolKind;
    assert_eq!(
        sketch_mode(&h).unwrap().tools.choice(),
        Some(Choice::Draw(ToolKind::CornerRectangle))
    );
    assert!(
        h.get_by_role_and_label(Role::Group, "Sketch Palette")
            .query_by_label("Corner Rectangle")
            .is_some()
    );
    // Pressed at (10, 10), dragged, and released at (50, 30).
    sketch_events(&f, &mut h, &dragged([10.0, 10.0], [50.0, 30.0]));
    settled(&mut h);
    let sketch = sketch_of_text(&f, &h, id.as_str());
    assert_eq!((sketch.points.len(), sketch.lines.len()), (4, 4));
    assert_eq!(sketch.constraints.len(), 4);
    assert_rectangle_in(&sketch, [10.0, 10.0], [50.0, 30.0]);
    assert_eq!(adds_sent(&h), 1);
    read_with_lines(&mut h, 4);
    // Then one by two clicks, at (60, 10) and (80, 20).
    sketch_events(&f, &mut h, &clicked(60.0, 10.0));
    sketch_events(&f, &mut h, &clicked(80.0, 20.0));
    settled(&mut h);
    let sketch = sketch_of_text(&f, &h, id.as_str());
    assert_eq!((sketch.points.len(), sketch.lines.len()), (8, 8));
    assert_eq!(sketch.constraints.len(), 8);
    assert_rectangle_in(&sketch, [10.0, 10.0], [50.0, 30.0]);
    assert_rectangle_in(&sketch, [60.0, 10.0], [80.0, 20.0]);
    assert_eq!(adds_sent(&h), 2);
    read_with_lines(&mut h, 8);
    // Finish keeps them.
    leave_sketch(&mut h, true);
    assert_eq!(sketch_of_text(&f, &h, id.as_str()).lines.len(), 8);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_line_chain_ends_by_enter_esc_leaves_the_tool_then_the_selection_and_its_letter_toggles_it() {
    use silico_app::sketch::tools::Choice;
    use silico_core::sketch_tools::ToolKind;
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    select_member(&mut h, "nb000000", silico_core::document::ElementKind::Line);
    let tool = |h: &AppHarness| sketch_mode(h).unwrap().tools.choice();
    // L, twice: on, then off.
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    assert_eq!(tool(&h), Some(Choice::Draw(ToolKind::Line)));
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    assert_eq!(tool(&h), None);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    // A chain from the triangle's corner pb000000 (20, 0) to (40, 0): one line, its start
    // the corner, shared; `horizontal`, as it's level.
    sketch_events(&f, &mut h, &clicked(20.2, 0.0));
    sketch_events(&f, &mut h, &clicked(40.0, 0.0));
    settled(&mut h);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let end = point_at(&sketch, 40.0, 0.0);
    let side = line_between(&sketch, "pb000000", &end).unwrap();
    assert!(holds_direction(&sketch, &side, true));
    assert_eq!(sketch.points.len(), 4);
    assert!(sketch_mode(&h).unwrap().tools.busy());
    // Enter ends the chain, the tool kept.
    h.key_press(eframe::egui::Key::Enter);
    step(&mut h);
    assert!(!sketch_mode(&h).unwrap().tools.busy());
    assert_eq!(tool(&h), Some(Choice::Draw(ToolKind::Line)));
    // X turns Construction on and off.
    h.key_press(eframe::egui::Key::X);
    step(&mut h);
    assert!(sketch_mode(&h).unwrap().settings.construction);
    h.key_press(eframe::egui::Key::X);
    step(&mut h);
    assert!(!sketch_mode(&h).unwrap().settings.construction);
    // Esc, the nearest thing first: the tool (the selection kept), then the selection.
    sketch_events(&f, &mut h, &clicked(60.0, 30.0));
    assert!(sketch_mode(&h).unwrap().tools.busy());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert_eq!(tool(&h), None);
    assert!(!sketch_mode(&h).unwrap().selection.is_empty());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(sketch_mode(&h).unwrap().selection.is_empty());
    assert!(sketch_mode(&h).is_some());
    // Nothing more was added: the click Esc ended was a chain's first.
    settled(&mut h);
    assert_eq!(adds_sent(&h), 1);
    assert_eq!(never_waits::waits(), 0);
}

/// A sketch of 511 points, whose 1022 unknowns leave room for one more point and no more
/// (the gate takes 1024 at most).
fn crowded() -> String {
    let points: Vec<String> = (0..511)
        .map(|i| format!(r#"{{"id": "p{i:07}", "x": {i}, "y": 50, "construction": false}}"#))
        .collect();
    format!(
        r#"{{
  "silico": 1,
  "units": {{"length": "mm", "resolution": "1nm"}},
  "naming": 5,
  "features": [
    {{"id": "s0000001", "type": "sketch", "v": 1, "name": "Crowded", "plane": "xy", "points": [{}], "lines": [], "circles": [], "arcs": [], "constraints": []}}
  ]
}}
"#,
        points.join(", ")
    )
}

#[test]
fn a_refused_add_is_said_and_forgotten_so_its_chain_ends() {
    let f = fixture();
    let text = crowded();
    let (mut h, _) = f.opened("crowded.silico", &text);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Crowded"));
    double_click(&mut h, "Browser", "Crowded");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    // A segment from (10, 10) to (30, 15): two new points, four unknowns, past the gate's
    // 1024. Refused, said; the document as it was.
    sketch_events(&f, &mut h, &clicked(10.0, 10.0));
    sketch_events(&f, &mut h, &clicked(30.0, 15.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 1);
    let said = h.state().tabs()[0].message.clone().unwrap_or_default();
    assert!(said.contains("1024"), "{said}");
    assert!(anywhere(&h, &said));
    assert_eq!(text_of(&f, &h), text);
    // The chain's last point was the refused add's: forgotten, so the chain ended there,
    // and the next click begins one, sending nothing.
    sketch_events(&f, &mut h, &clicked(50.0, 15.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_add_another_client_undoes_is_forgotten_once_a_read_without_it_arrives() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    sketch_events(&f, &mut h, &clicked(30.0, 30.0));
    sketch_events(&f, &mut h, &clicked(40.0, 35.0));
    settled(&mut h);
    read_with_lines(&mut h, 4);
    // Another client undoes it.
    let id = h.state().tabs()[0].id;
    let revision = h.state().tabs()[0].revision;
    assert!(f.sessions.undo(&id, revision, None).wait().result.is_ok());
    assert_eq!(text_of(&f, &h), SKETCHED);
    read_with_lines(&mut h, 3);
    // The chain's last point, (40, 35), is gone: the next click begins a chain, sending
    // nothing; the one after draws from it, a line from (50, 35) to (60, 45).
    sketch_events(&f, &mut h, &clicked(50.0, 35.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 1);
    sketch_events(&f, &mut h, &clicked(60.0, 45.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 2);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let (a, b) = (point_at(&sketch, 50.0, 35.0), point_at(&sketch, 60.0, 45.0));
    assert!(line_between(&sketch, &a, &b).is_some());
    assert_eq!(sketch.lines.len(), 4);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn automatic_relations_off_in_the_palette_adds_no_horizontal() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    h.get_by_role_and_label(Role::Group, "Sketch Palette")
        .get_by_label("Automatic relations")
        .click();
    step(&mut h);
    assert!(!sketch_mode(&h).unwrap().settings.automatic_relations);
    // (30, 30) to (50, 30.3), 0.86° off level: with the setting on it would be put level and
    // held `horizontal`; off, it's where the pointer left it, with nothing.
    sketch_events(&f, &mut h, &dragged([30.0, 30.0], [50.0, 30.3]));
    settled(&mut h);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let (a, b) = (point_at(&sketch, 30.0, 30.0), point_at(&sketch, 50.0, 30.3));
    assert!(line_between(&sketch, &a, &b).is_some());
    assert!(sketch.constraints.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

/// Two lines crossing at (20, 0): na000001 from pa000001 (0, 0) to pa000002 (40, 0), and
/// na000002 from pa000003 (20, −10) to pa000004 (20, 10).
const CROSSED: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Cross", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 40, "y": 0, "construction": false}, {"id": "pa000003", "x": 20, "y": -10, "construction": false}, {"id": "pa000004", "x": 20, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000003", "to": "pa000004", "construction": false}], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

#[test]
fn trim_to_closest_and_split_send_their_commands_and_their_results_read_back() {
    use silico_core::document::{Direction, Form, Reference};
    let f = fixture();
    let (mut h, _) = f.opened("crossed.silico", CROSSED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Cross"));
    double_click(&mut h, "Browser", "Cross");
    sketch_read(&mut h);
    // T, then Trim to closest in the palette.
    h.key_press(eframe::egui::Key::T);
    step(&mut h);
    h.get_by_role_and_label(Role::Group, "Sketch Palette")
        .get_by_label("Trim to closest")
        .click();
    step(&mut h);
    // A click 0.3 mm off na000001 at x = 30: the piece from the crossing (20, 0) to its end
    // goes, pa000002 with it; its new end, a point at (20, 0), held `on` na000002.
    sketch_events(&f, &mut h, &clicked(30.0, 0.3));
    settled(&mut h);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    assert!(sketch.points.iter().all(|p| p.id.as_str() != "pa000002"));
    let cut = point_at(&sketch, 20.0, 0.0);
    assert_eq!(
        line_between(&sketch, "pa000001", &cut).as_deref(),
        Some("na000001")
    );
    assert_eq!(sketch.lines.len(), 2);
    assert!(matches!(
        sketch.constraints.as_slice(),
        [c] if c.form == Form::On {
            point: Reference::Element(silico_core::document::ElementId::new(&cut).unwrap()),
            curve: Reference::Element(silico_core::document::ElementId::new("na000002").unwrap()),
        }
    ));
    assert!(
        h.state().tabs()[0]
            .sent
            .contains(&Sent::SketchEdit("sketch_trim"))
    );
    read_with_lines(&mut h, 2);
    // Split: a click 0.3 mm off na000002 at y = 5 splits it at (20, 5), on it: na000002 to
    // a new point there, a new line on from it to pa000004, held on one line by a
    // `tangent_at` (`same`).
    h.state_mut().tabs_mut()[0].sketch_choose(Some(silico_app::sketch::tools::Choice::Split));
    sketch_events(&f, &mut h, &clicked(20.3, 5.0));
    settled(&mut h);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let split = point_at(&sketch, 20.0, 5.0);
    assert_eq!(
        line_between(&sketch, "pa000003", &split).as_deref(),
        Some("na000002")
    );
    let piece = line_between(&sketch, &split, "pa000004").unwrap();
    assert!(sketch.constraints.iter().any(|c| c.form
        == Form::TangentAt {
            a: Reference::Element(silico_core::document::ElementId::new("na000002").unwrap()),
            b: Reference::Element(silico_core::document::ElementId::new(&piece).unwrap()),
            at: Reference::Element(silico_core::document::ElementId::new(&split).unwrap()),
            direction: Direction::Same,
        }));
    read_with_lines(&mut h, 3);
    // A split at na000001's end, pa000001, is the core's to refuse: said, nothing changed.
    let before = text_of(&f, &h);
    sketch_events(&f, &mut h, &clicked(0.0, 0.0));
    settled(&mut h);
    assert_eq!(text_of(&f, &h), before);
    let said = h.state().tabs()[0].message.clone().unwrap_or_default();
    assert!(said.contains("end"), "{said}");
    assert!(anywhere(&h, &said));
    assert!(
        h.state().tabs()[0]
            .sent
            .contains(&Sent::SketchEdit("sketch_split"))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn power_trim_extends_a_line_dragged_along_to_the_pointer() {
    let f = fixture();
    let (mut h, _) = f.opened("crossed.silico", CROSSED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Cross"));
    double_click(&mut h, "Browser", "Cross");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::T);
    step(&mut h);
    // Pressed on na000001 at (30, 0) and dragged along it to (45, 0.2): its end, pa000002,
    // goes to (45, 0), the pointer put on its line (`sketch_extend` to a point).
    sketch_events(&f, &mut h, &dragged([30.0, 0.0], [45.0, 0.2]));
    settled(&mut h);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    assert_eq!(point_at(&sketch, 45.0, 0.0), "pa000002");
    assert!(
        h.state().tabs()[0]
            .sent
            .contains(&Sent::SketchEdit("sketch_extend"))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tools_add_undone_inside_sketch_mode_is_forgotten_once_a_read_without_it_arrives() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    sketch_events(&f, &mut h, &clicked(30.0, 30.0));
    sketch_events(&f, &mut h, &clicked(40.0, 35.0));
    settled(&mut h);
    read_with_lines(&mut h, 4);
    // Its own Undo, the chain still under way: the add answered at the revision it made, so
    // the read of the undo's revision, without it, ends the chain there.
    chord(&mut h, eframe::egui::Key::Z);
    assert_eq!(text_of(&f, &h), SKETCHED);
    read_with_lines(&mut h, 3);
    sketch_events(&f, &mut h, &clicked(50.0, 35.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_rectangle_then_another_clients_edit_are_both_undone_by_cancel() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    // An edit before sketch mode, which Cancel keeps.
    select(&mut h, "Plate");
    type_name(&mut h, "K");
    settled(&mut h);
    let before = text_of(&f, &h);
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::R);
    step(&mut h);
    sketch_events(&f, &mut h, &dragged([30.0, 30.0], [50.0, 40.0]));
    settled(&mut h);
    assert_eq!(sketch_of_text(&f, &h, "s0000001").lines.len(), 7);
    let moved = other_client_renames(&f, &h, "b0000002", "Spacer");
    step_until(&mut h, "the other client's revision", |h| {
        h.state().tabs()[0].revision == moved
    });
    assert!(text_of(&f, &h).contains(r#""name": "Spacer""#));
    // Cancel: the rectangle gone, and the other client's edit with it; the rename before
    // sketch mode kept.
    leave_sketch(&mut h, false);
    assert_eq!(text_of(&f, &h), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_chain_clicked_while_its_reads_are_held_goes_on_from_each_segments_end() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    settled(&mut h);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    // Every solve held: each add is applied, but no rebuild publishes it, so the read the
    // tool works against stays the triangle's. Each segment's start is found from the add
    // before it, kept until a read of the revision it made says otherwise.
    f.sketches.hold();
    let published = h.state().tabs()[0].published;
    for [x, y] in [[30.0, 30.0], [40.0, 35.0], [50.0, 30.0], [60.0, 45.0]] {
        sketch_events(&f, &mut h, &clicked(x, y));
        settled(&mut h);
    }
    assert_eq!(h.state().tabs()[0].published, published);
    assert_eq!(
        sketch_mode(&h)
            .unwrap()
            .drawable()
            .unwrap()
            .members
            .lines
            .len(),
        3
    );
    assert_eq!(adds_sent(&h), 3);
    f.sketches.release();
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let [a, b, c, d] = [[30.0, 30.0], [40.0, 35.0], [50.0, 30.0], [60.0, 45.0]]
        .map(|[x, y]| point_at(&sketch, x, y));
    assert!(line_between(&sketch, &a, &b).is_some());
    assert!(line_between(&sketch, &b, &c).is_some());
    assert!(line_between(&sketch, &c, &d).is_some());
    assert_eq!((sketch.points.len(), sketch.lines.len()), (7, 6));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_with_a_chain_under_way_and_a_rebuild_held_ends_the_chain_not_the_rebuild() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    settled(&mut h);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    // A segment, whose rebuild the fake's held solve keeps running, and the chain going on.
    f.sketches.hold();
    sketch_events(&f, &mut h, &clicked(30.0, 30.0));
    sketch_events(&f, &mut h, &clicked(40.0, 35.0));
    settled(&mut h);
    step_until(&mut h, "the rebuild running", |h| {
        h.state().tabs()[0].status.as_ref().is_some_and(|status| {
            status.rebuild.state == silico_core::command::RebuildState::Running
        })
    });
    assert!(sketch_mode(&h).unwrap().tools.busy());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert_eq!(sketch_mode(&h).unwrap().tools.choice(), None);
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(!rebuild_stopped(&h));
    assert!(h.state().tabs()[0].stoppable());
    f.sketches.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tools_adds_with_no_sketch_to_send_them_to_are_refused_to_it() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    // A segment drawn by sketch mode's frame alone, its add left with the tool; then the
    // sketch unknown when the tab takes it.
    let events: Vec<_> = clicked(30.0, 30.0)
        .into_iter()
        .chain(clicked(40.0, 35.0))
        .collect();
    {
        let mode = h.state_mut().tabs_mut()[0].sketch.as_mut().unwrap();
        let ctx = eframe::egui::Context::default();
        let mut output = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            let input = silico_app::sketch::FrameInput {
                events: &events,
                pointer: None,
                project: &to_screen,
                to_plane: &to_sketch,
            };
            mode.frame(&ui.painter().clone(), &input, &mut None);
        });
        output.textures_delta.clear();
        mode.sketch = None;
    }
    step(&mut h);
    assert_eq!(adds_sent(&h), 0);
    // Refused to the tool: the chain's last point, the add's, is forgotten, so with the
    // sketch known again the next click begins a chain and sends nothing.
    h.state_mut().tabs_mut()[0].sketch.as_mut().unwrap().sketch = Some(sketch_id("s0000001"));
    sketch_events(&f, &mut h, &clicked(50.0, 30.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 0);
    assert_eq!(text_of(&f, &h), SKETCHED);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tools_letter_with_shift_or_alt_chooses_nothing() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    for modifiers in [eframe::egui::Modifiers::SHIFT, eframe::egui::Modifiers::ALT] {
        for key in [
            eframe::egui::Key::L,
            eframe::egui::Key::T,
            eframe::egui::Key::X,
        ] {
            h.key_press_modifiers(modifiers, key);
            step(&mut h);
            let mode = sketch_mode(&h).unwrap();
            assert_eq!(mode.tools.choice(), None, "{modifiers:?} {key:?}");
            assert!(!mode.settings.construction, "{modifiers:?} {key:?}");
        }
    }
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    assert!(sketch_mode(&h).unwrap().tools.choice().is_some());
    assert_eq!(never_waits::waits(), 0);
}

// ---- Smart Dimension and drags (M5.11c; section 9, *Smart Dimension*, *A dimension that
// over-defines*, *Dragging*) ----

/// The sketch solver answering as PlaneGCS counts a sketch each of whose constraints is one
/// equation, as each relation the tools add and each length is: its freedoms its unknowns
/// less its constraints, every unknown free while any is; past none, every length in
/// conflict (as `drag_begin`'s rectangle given a second width reads −1). A length of 99 mm
/// conflicts wherever it is, a value no sketch here can take. Values answered as sent.
fn counting(f: &Fixture) {
    use silico_protocol::{Solved, SolverConstraint, Tag};
    f.sketches.answer(|system| {
        let sent = FakeSketches::as_sent(system);
        let equations = system
            .constraints
            .iter()
            .filter(|c| c.tag().is_some())
            .count();
        let dofs = system.unknowns.len() as i32 - equations as i32;
        let lengths: Vec<(Tag, Option<f64>)> = system
            .constraints
            .iter()
            .filter_map(|c| match c {
                SolverConstraint::Distance { tag, value, .. } => {
                    Some((*tag, system.values.get(value.0 as usize).copied()))
                }
                _ => None,
            })
            .collect();
        let impossible: Vec<Tag> = lengths
            .iter()
            .filter(|(_, v)| *v == Some(99.0))
            .map(|(t, _)| *t)
            .collect();
        Ok(if dofs < 0 {
            Solved {
                dofs,
                conflicting: lengths.iter().map(|(t, _)| *t).collect(),
                ..sent
            }
        } else if !impossible.is_empty() {
            Solved {
                dofs,
                conflicting: impossible,
                ..sent
            }
        } else if dofs > 0 {
            Solved {
                dofs,
                free: system.unknowns.clone(),
                ..sent
            }
        } else {
            sent
        })
    });
}

/// Sketch mode's newest read's state and freedoms.
fn sketch_state(h: &AppHarness) -> Option<(silico_core::sketch_states::SketchState, i32)> {
    sketch_mode(h)
        .and_then(|mode| mode.drawable())
        .map(|read| (read.states.state, read.states.dofs))
}

/// Stepped until sketch mode's newest read has `constraints` constraints, and Smart
/// Dimension (if it's on) has settled on it.
fn read_with_constraints(h: &mut AppHarness, constraints: usize) {
    step_until(h, "the read after the dimension", |h| {
        sketch_mode(h).is_some_and(|mode| {
            mode.drawable()
                .is_some_and(|read| read.members.constraints.len() == constraints)
                && (!mode.tools.busy() || mode.offer().is_some())
        })
    });
}

/// The Modify box asked `action`, as its field or buttons ask it.
fn modify(f: &Fixture, h: &mut AppHarness, action: silico_app::sketch::dimension::ModifyAction) {
    let sessions = Arc::clone(&f.sessions);
    {
        let _marked = never_waits::marked();
        h.state_mut().tabs_mut()[0].sketch_modify(&sessions, action);
    }
    step(h);
}

/// Smart Dimension's picks at `pick` (a side), then its click at `at`, and the Modify box
/// open holding `measured`.
fn dimension_at(f: &Fixture, h: &mut AppHarness, pick: [f64; 2], at: [f64; 2], measured: &str) {
    sketch_events(f, h, &clicked(pick[0], pick[1]));
    sketch_events(f, h, &clicked(at[0], at[1]));
    step_until(h, "the Modify box open", |h| {
        sketch_mode(h)
            .and_then(|mode| mode.modify_view())
            .is_some_and(|view| view.text == measured)
    });
}

/// The constraint of `sketch` with the ID `id`.
fn constraint_of<'a>(
    sketch: &'a silico_core::document::Sketch,
    id: &str,
) -> &'a silico_core::document::SketchConstraint {
    sketch
        .constraints
        .iter()
        .find(|c| c.id.as_str() == id)
        .unwrap()
}

/// A length's line, value as written, and whether it drives.
fn length_parts(form: &silico_core::document::Form) -> Option<(String, String, bool)> {
    match form {
        silico_core::document::Form::Length {
            line: silico_core::document::Reference::Element(line),
            value,
            driving,
        } => Some((line.as_str().to_owned(), value.to_string(), *driving)),
        _ => None,
    }
}

/// The lengths of `sketch`, by ID, each with its line, value, and whether it drives.
fn lengths(sketch: &silico_core::document::Sketch) -> Vec<(String, String, String, bool)> {
    sketch
        .constraints
        .iter()
        .filter_map(|c| length_parts(&c.form).map(|(l, v, d)| (c.id.as_str().to_owned(), l, v, d)))
        .collect()
}

/// Create Sketch on XY, a corner rectangle from the origin to (40, 20) drawn with R, and
/// Smart Dimension (D) placing the right side's length and the top's, each its measured
/// value: the sketch's ID, the right side's, and the top's.
fn fully_defined(f: &Fixture, h: &mut AppHarness) -> (String, String, String) {
    use silico_app::sketch::dimension::ModifyAction;
    use silico_core::sketch_states::SketchState;
    step_until(h, "its features", |h| shows(h, "Timeline", "Hole"));
    create_sketch(h, "XY plane");
    sketch_read(h);
    let id = sketch_mode(h).unwrap().sketch.clone().unwrap();
    h.key_press(eframe::egui::Key::R);
    step(h);
    sketch_events(f, h, &dragged([0.0, 0.0], [40.0, 20.0]));
    settled(h);
    read_with_lines(h, 4);
    let sketch = sketch_of_text(f, h, id.as_str());
    // Worked by hand: four corners, eight unknowns; the rectangle's two horizontals and two
    // verticals, and the origin's corner held level with it and plumb with it (#278's A):
    // six equations, two freedoms left, its width and its height.
    assert_eq!(sketch.constraints.len(), 6, "{:?}", sketch.constraints);
    assert_eq!(sketch_state(h), Some((SketchState::UnderDefined, 2)));
    let corner = |x, y| point_at(&sketch, x, y);
    let right = line_between(&sketch, &corner(40.0, 0.0), &corner(40.0, 20.0)).unwrap();
    let top = line_between(&sketch, &corner(40.0, 20.0), &corner(0.0, 20.0)).unwrap();
    // D: Smart Dimension (DESIGN.md, *Keyboard*).
    h.key_press(eframe::egui::Key::D);
    step(h);
    assert_eq!(
        sketch_mode(h).unwrap().tools.choice(),
        Some(silico_app::sketch::tools::Choice::Dimension)
    );
    assert!(
        h.get_by_role_and_label(Role::Group, "Sketch Palette")
            .query_by_label("Smart Dimension")
            .is_some()
    );
    // The right side picked at (40, 10) and placed at (55, 10): its 20, accepted.
    dimension_at(f, h, [40.0, 10.0], [55.0, 10.0], "20");
    modify(f, h, ModifyAction::Accept);
    settled(h);
    read_with_constraints(h, 7);
    assert_eq!(sketch_state(h), Some((SketchState::UnderDefined, 1)));
    // The top picked at (20, 20) and placed at (20, 30): its 40.
    dimension_at(f, h, [20.0, 20.0], [20.0, 30.0], "40");
    modify(f, h, ModifyAction::Accept);
    settled(h);
    read_with_constraints(h, 8);
    (id.as_str().to_owned(), right, top)
}

#[test]
fn smart_dimension_on_two_sides_fully_defines_a_rectangle_drawn_from_the_origin_and_its_colour_says_so()
 {
    use silico_app::sketch::draw::{Drawn, StrokeKind};
    use silico_app::sketch::style::SKETCH_FULLY_DEFINED;
    use silico_core::sketch_states::SketchState;
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    let (id, right, top) = fully_defined(&f, &mut h);
    assert_eq!(sketch_state(&h), Some((SketchState::FullyDefined, 0)));
    // Two lengths placed, driving, at the measured values: the right side 20, the top 40.
    let sketch = sketch_of_text(&f, &h, &id);
    let placed: Vec<(String, String, bool)> = lengths(&sketch)
        .into_iter()
        .map(|(_, line, value, driving)| (line, value, driving))
        .collect();
    let mut expected = vec![(right, "20".to_owned(), true), (top, "40".to_owned(), true)];
    let mut placed_sorted = placed.clone();
    placed_sorted.sort();
    expected.sort();
    assert_eq!(placed_sorted, expected);
    assert_eq!(
        h.state().tabs()[0]
            .sent
            .iter()
            .filter(|sent| **sent == Sent::SketchEdit("sketch_add"))
            .count(),
        3
    );
    // Drawn from the read: every curve and point in the fully-defined colour.
    sketch_events(&f, &mut h, &[]);
    let drawn = sketch_mode(&h).unwrap().last_drawn();
    let members: Vec<&Drawn> = drawn
        .iter()
        .filter(|d| {
            matches!(
                d,
                Drawn::Stroke {
                    kind: StrokeKind::Curve { .. },
                    ..
                } | Drawn::Point { .. }
            )
        })
        .collect();
    assert_eq!(members.len(), 8);
    assert!(members.iter().all(|d| d.colour() == SKETCH_FULLY_DEFINED));
    // The palette says so: in its status, besides its legend's line.
    assert_eq!(
        h.get_by_role_and_label(Role::Group, "Sketch Palette")
            .query_all_by_label("Fully defined")
            .count(),
        2
    );
    assert_eq!(never_waits::waits(), 0);
}

/// The over-define prompt, if it's shown.
fn prompt(h: &AppHarness) -> Option<egui_kittest::Node<'_>> {
    h.query_by_role_and_label(Role::Dialog, "Dimension over-defines the sketch")
}

/// The over-define prompt's choice labelled `label` clicked, once the prompt is laid out
/// (a modal is centred, so it moves as it's first sized).
fn choose(h: &mut AppHarness, label: &str) {
    step_until(h, "the prompt", |h| prompt(h).is_some());
    let place = |h: &AppHarness| {
        prompt(h)
            .and_then(|p| p.query_by_role_and_label(Role::Button, label))
            .map(|button| button.accesskit_node().bounding_box())
    };
    let last = std::cell::Cell::new(place(h));
    step_until(h, "the prompt laid out", |h| {
        let now = place(h);
        now.is_some() && now == last.replace(now)
    });
    prompt(h)
        .unwrap()
        .get_by_role_and_label(Role::Button, label)
        .click();
    step(h);
}

/// A third dimension on the right side, placed at (`x`, 10) with `typed`, over-defining
/// it: stepped until its read, of `constraints` constraints, offers the three choices.
fn over_define(f: &Fixture, h: &mut AppHarness, x: f64, typed: &str, constraints: usize) -> String {
    use silico_app::sketch::dimension::ModifyAction;
    dimension_at(f, h, [40.0, 10.0], [x, 10.0], "20");
    modify(f, h, ModifyAction::Typed(typed.to_owned()));
    modify(f, h, ModifyAction::Accept);
    settled(h);
    read_with_constraints(h, constraints);
    step_until(h, "the offer", |h| {
        sketch_mode(h).is_some_and(|mode| mode.offer().is_some())
    });
    sketch_mode(h)
        .unwrap()
        .offer()
        .unwrap()
        .constraint
        .as_str()
        .to_owned()
}

#[test]
fn a_conflicting_dimension_is_placed_reads_over_defined_naming_them_and_driven_cancel_leave_and_overtaken_do_as_sd7_says()
 {
    use silico_core::document::FeatureId;
    use silico_core::sketch_states::{ConstraintState, SketchState};
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    let (id, right, _) = fully_defined(&f, &mut h);
    let before_any = text_of(&f, &h);
    // A second length of the right side, 30 where the first holds 20: placed, not refused.
    let third = over_define(&f, &mut h, 60.0, "30", 9);
    let sketch = sketch_of_text(&f, &h, &id);
    assert_eq!(
        length_parts(&constraint_of(&sketch, &third).form),
        Some((right.clone(), "30".to_owned(), true))
    );
    // Over defined, naming the three lengths in conflict (nine equations, eight unknowns).
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    assert_eq!(read.states.state, SketchState::OverDefined);
    let mut conflicting: Vec<String> = read
        .states
        .constraints
        .iter()
        .filter(|(_, s)| **s == ConstraintState::Conflicting)
        .map(|(id, _)| id.as_str().to_owned())
        .collect();
    conflicting.sort();
    let mut all_lengths: Vec<String> = lengths(&sketch).into_iter().map(|l| l.0).collect();
    all_lengths.sort();
    assert_eq!(conflicting, all_lengths);
    // The prompt names them, and offers the three.
    step_until(&mut h, "the prompt", |h| prompt(h).is_some());
    let shown = everything(&h).join(" | ");
    for length in &all_lengths {
        assert!(shown.contains(length.as_str()), "{length} in {shown}");
    }
    for choice in ["Make it driven", "Leave it driving", "Cancel"] {
        assert!(
            prompt(&h)
                .unwrap()
                .query_by_role_and_label(Role::Button, choice)
                .is_some(),
            "{choice}"
        );
    }
    // Driven: `set_field` of its `driving` to false, which solves the sketch again.
    choose(&mut h, "Make it driven");
    settled(&mut h);
    let field = format!("constraints.{third}.driving");
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Edit(field)));
    assert_eq!(
        length_parts(&constraint_of(&sketch_of_text(&f, &h, &id), &third).form),
        Some((right.clone(), "30".to_owned(), false))
    );
    step_until(&mut h, "the sketch solved", |h| {
        sketch_state(h) == Some((SketchState::FullyDefined, 0)) && prompt(h).is_none()
    });
    // Cancel (Esc, the prompt's key): `undo`, the text as it was before the add, byte for byte.
    let before = text_of(&f, &h);
    over_define(&f, &mut h, 65.0, "35", 10);
    step_until(&mut h, "the prompt", |h| prompt(h).is_some());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Undo(None)));
    assert_eq!(text_of(&f, &h), before);
    step_until(&mut h, "the prompt closed", |h| prompt(h).is_none());
    // Leave: nothing sent; the sketch over defined, the dimension kept, driving.
    read_with_constraints(&mut h, 9);
    let left = over_define(&f, &mut h, 70.0, "36", 10);
    let last = h.state().tabs()[0].sent.last().cloned();
    let asked = |h: &AppHarness| {
        h.state().tabs()[0]
            .sent
            .iter()
            .filter(|sent| {
                matches!(sent, Sent::Undo(_))
                    || **sent == Sent::Edit(format!("constraints.{left}.driving"))
            })
            .count()
    };
    let before = asked(&h);
    choose(&mut h, "Leave it driving");
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].sent.last().cloned(), last);
    assert_eq!(asked(&h), before);
    assert!(prompt(&h).is_none());
    assert_eq!(
        sketch_state(&h).map(|s| s.0),
        Some(SketchState::OverDefined)
    );
    assert_eq!(
        length_parts(&constraint_of(&sketch_of_text(&f, &h, &id), &left).form),
        Some((right.clone(), "36".to_owned(), true))
    );
    // Overtaken: another, offered, then another client's edit lands first; Driven, made
    // against the add's revision, is refused as a conflict, said, and never sent again.
    let last = over_define(&f, &mut h, 75.0, "37", 11);
    let session = h.state().tabs()[0].id;
    let revision = f.sessions.status(&session).session.unwrap().revision;
    let renamed = f
        .sessions
        .set_field(
            &session,
            revision,
            FeatureId::new(&id).unwrap(),
            "name".to_owned(),
            json::read(br#""Renamed""#).unwrap(),
        )
        .wait();
    assert!(renamed.result.is_ok(), "{:?}", renamed.result);
    {
        let sessions = Arc::clone(&f.sessions);
        let _marked = never_waits::marked();
        h.state_mut().tabs_mut()[0]
            .sketch_offer(&sessions, silico_core::sketch_tools::OfferChoice::Driven);
    }
    step_until(&mut h, "the choice overtaken", |h| {
        h.state().tabs()[0].message.as_deref() == Some(silico_app::tab::OVERTAKEN)
    });
    for _ in 0..20 {
        step(&mut h);
    }
    let field = Sent::Edit(format!("constraints.{last}.driving"));
    assert_eq!(
        h.state().tabs()[0]
            .sent
            .iter()
            .filter(|sent| **sent == field)
            .count(),
        1
    );
    assert!(prompt(&h).is_none());
    assert_eq!(
        length_parts(&constraint_of(&sketch_of_text(&f, &h, &id), &last).form),
        Some((right, "37".to_owned(), true))
    );
    // Cancel overtaken the same way: its undo, against the add's revision, refused as a
    // conflict, said, sent once, and the dimension kept.
    let cancelled = over_define(&f, &mut h, 80.0, "38", 12);
    let revision = f.sessions.status(&session).session.unwrap().revision;
    let renamed = f
        .sessions
        .set_field(
            &session,
            revision,
            FeatureId::new(&id).unwrap(),
            "name".to_owned(),
            json::read(br#""Renamed again""#).unwrap(),
        )
        .wait();
    assert!(renamed.result.is_ok(), "{:?}", renamed.result);
    let undos = |h: &AppHarness| {
        h.state().tabs()[0]
            .sent
            .iter()
            .filter(|sent| **sent == Sent::Undo(None))
            .count()
    };
    let before = undos(&h);
    {
        let sessions = Arc::clone(&f.sessions);
        let _marked = never_waits::marked();
        h.state_mut().tabs_mut()[0]
            .sketch_offer(&sessions, silico_core::sketch_tools::OfferChoice::Cancel);
    }
    assert_eq!(h.state().tabs()[0].message, None);
    step_until(&mut h, "the cancel overtaken", |h| {
        h.state().tabs()[0].message.as_deref() == Some(silico_app::tab::OVERTAKEN)
    });
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(undos(&h), before + 1);
    assert!(
        sketch_of_text(&f, &h, &id)
            .constraints
            .iter()
            .any(|c| c.id.as_str() == cancelled)
    );
    assert_ne!(text_of(&f, &h), before_any);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_modify_boxs_text_typed_reaches_the_core_as_typed_and_its_refusal_is_said_in_the_box() {
    use silico_core::sketch_tools::Message;
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create_sketch(&mut h, "XY plane");
    sketch_read(&mut h);
    let id = sketch_mode(&h).unwrap().sketch.clone().unwrap();
    h.key_press(eframe::egui::Key::R);
    step(&mut h);
    sketch_events(&f, &mut h, &dragged([0.0, 0.0], [40.0, 20.0]));
    settled(&mut h);
    read_with_lines(&mut h, 4);
    h.key_press(eframe::egui::Key::D);
    step(&mut h);
    let field = |h: &AppHarness| -> Option<String> {
        h.query_by_role_and_label(Role::Dialog, "Modify dimension")?
            .query_by_label("Value in millimetres")?
            .value()
    };
    // Typed into the box itself, which has the keyboard: Home, its value's two characters
    // deleted, and what's typed, in one frame.
    let typed = |h: &mut AppHarness, text: &str| {
        for key in [
            eframe::egui::Key::Home,
            eframe::egui::Key::Delete,
            eframe::egui::Key::Delete,
        ] {
            h.input_mut().events.push(eframe::egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: eframe::egui::Modifiers::NONE,
            });
        }
        h.query_by_role_and_label(Role::Dialog, "Modify dimension")
            .unwrap()
            .get_by_label("Value in millimetres")
            .type_text(text);
        step(h);
        step(h);
    };
    dimension_at(&f, &mut h, [40.0, 10.0], [55.0, 10.0], "20");
    step_until(&mut h, "the box drawn", |h| field(h).is_some());
    typed(&mut h, "25mm");
    assert_eq!(field(&h).as_deref(), Some("25mm"));
    assert_eq!(
        sketch_mode(&h)
            .unwrap()
            .tools
            .dimension_preview()
            .and_then(|p| p.modify.clone())
            .as_deref(),
        Some("25mm")
    );
    h.key_press(eframe::egui::Key::Enter);
    step(&mut h);
    step(&mut h);
    // The core read no number of it: said in the box, nothing added, the box still open.
    let refused = Message::NotANumber.text();
    step_until(&mut h, "the refusal said", |h| {
        h.query_by_role_and_label(Role::Dialog, "Modify dimension")
            .is_some_and(|d| d.query_by_label(&refused).is_some())
    });
    assert_eq!(sketch_of_text(&f, &h, id.as_str()).constraints.len(), 6);
    // Its field takes at most 256 characters: 300 typed after its four, 252 of them kept.
    h.input_mut().events.push(eframe::egui::Event::Key {
        key: eframe::egui::Key::End,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: eframe::egui::Modifiers::NONE,
    });
    h.query_by_role_and_label(Role::Dialog, "Modify dimension")
        .unwrap()
        .get_by_label("Value in millimetres")
        .focus();
    step(&mut h);
    h.query_by_role_and_label(Role::Dialog, "Modify dimension")
        .unwrap()
        .get_by_label("Value in millimetres")
        .type_text(&"1".repeat(300));
    step(&mut h);
    step(&mut h);
    assert_eq!(field(&h).map(|text| text.chars().count()), Some(256));
    // " 25 ", spaces and all, as the core gets it: its reader takes them, 25.
    modify(
        &f,
        &mut h,
        silico_app::sketch::dimension::ModifyAction::Typed(" 25 ".to_owned()),
    );
    assert_eq!(
        sketch_mode(&h)
            .unwrap()
            .tools
            .dimension_preview()
            .and_then(|p| p.modify.clone())
            .as_deref(),
        Some(" 25 ")
    );
    modify(
        &f,
        &mut h,
        silico_app::sketch::dimension::ModifyAction::Accept,
    );
    settled(&mut h);
    read_with_constraints(&mut h, 7);
    let sketch = sketch_of_text(&f, &h, id.as_str());
    assert_eq!(
        lengths(&sketch)
            .into_iter()
            .map(|l| l.2)
            .collect::<Vec<_>>(),
        ["25"]
    );
    // "1e1" on the top: the exponent read exactly, 10.
    dimension_at(&f, &mut h, [20.0, 20.0], [20.0, 30.0], "40");
    modify(
        &f,
        &mut h,
        silico_app::sketch::dimension::ModifyAction::Typed("1e1".to_owned()),
    );
    modify(
        &f,
        &mut h,
        silico_app::sketch::dimension::ModifyAction::Accept,
    );
    settled(&mut h);
    read_with_constraints(&mut h, 8);
    let mut values: Vec<String> = lengths(&sketch_of_text(&f, &h, id.as_str()))
        .into_iter()
        .map(|l| l.2)
        .collect();
    values.sort();
    assert_eq!(values, ["10", "25"]);
    assert_eq!(never_waits::waits(), 0);
}

/// The screen place of the value of the dimension `id`, as the last frame drew it.
fn label_of_dimension(h: &AppHarness, id: &str) -> eframe::egui::Pos2 {
    use silico_app::sketch::draw::Drawn;
    sketch_mode(h)
        .unwrap()
        .last_drawn()
        .iter()
        .find_map(|d| match d {
            Drawn::Label {
                of: Some(of), at, ..
            } if of.as_str() == id => Some(*at),
            _ => None,
        })
        .unwrap()
}

/// A double-click at `at` on the screen, as the pane gives one.
fn double_clicked_at(at: eframe::egui::Pos2) -> Vec<silico_app::sketch::PointerEvent> {
    use silico_app::sketch::PointerAction::{DoubleClick, Press, Release};
    [Press, Release, Press, Release, DoubleClick]
        .into_iter()
        .map(|action| silico_app::sketch::PointerEvent {
            action,
            at,
            shift: false,
            ctrl: false,
        })
        .collect()
}

#[test]
fn a_placed_dimension_double_clicked_is_set_as_typed_and_an_over_defining_edit_offers_the_three() {
    use silico_app::sketch::dimension::ModifyAction;
    use silico_core::sketch_states::SketchState;
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    let (id, right, _) = fully_defined(&f, &mut h);
    // Smart Dimension left (Esc with nothing picked), the right side's length found.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert_eq!(sketch_mode(&h).unwrap().tools.choice(), None);
    let sketch = sketch_of_text(&f, &h, &id);
    let length = lengths(&sketch)
        .into_iter()
        .find(|l| l.1 == right)
        .unwrap()
        .0;
    sketch_events(&f, &mut h, &[]);
    let at = label_of_dimension(&h, &length);
    sketch_events(&f, &mut h, &double_clicked_at(at));
    let edit = sketch_mode(&h).unwrap().edit_box.clone().unwrap();
    assert_eq!(
        (edit.constraint.as_str(), edit.text.as_str()),
        (length.as_str(), "20")
    );
    step_until(&mut h, "the box drawn", |h| {
        h.query_by_role_and_label(Role::Dialog, "Modify dimension")
            .is_some()
    });
    // Set to 25: `set_field` of its value, the text as typed; no offer.
    modify(&f, &mut h, ModifyAction::Typed("25".to_owned()));
    modify(&f, &mut h, ModifyAction::Accept);
    settled(&mut h);
    let field = format!("constraints.{length}.value");
    assert!(
        h.state().tabs()[0]
            .sent
            .contains(&Sent::Edit(field.clone()))
    );
    assert_eq!(
        length_parts(&constraint_of(&sketch_of_text(&f, &h, &id), &length).form),
        Some((right.clone(), "25".to_owned(), true))
    );
    step_until(&mut h, "the edit's read", |h| {
        sketch_mode(h).is_some_and(|mode| mode.edited.is_none() && mode.edit_box.is_none())
    });
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(prompt(&h).is_none());
    assert_eq!(sketch_state(&h), Some((SketchState::FullyDefined, 0)));
    // Set to 99, which no sketch here takes: over defined, so the three are offered.
    sketch_events(&f, &mut h, &[]);
    let at = label_of_dimension(&h, &length);
    sketch_events(&f, &mut h, &double_clicked_at(at));
    modify(&f, &mut h, ModifyAction::Typed("99".to_owned()));
    modify(&f, &mut h, ModifyAction::Accept);
    settled(&mut h);
    step_until(&mut h, "the edit's offer", |h| prompt(h).is_some());
    assert_eq!(
        sketch_mode(&h)
            .unwrap()
            .offer()
            .map(|o| o.constraint.as_str().to_owned()),
        Some(length.clone())
    );
    choose(&mut h, "Make it driven");
    settled(&mut h);
    assert_eq!(
        h.state().tabs()[0].sent.last(),
        Some(&Sent::Edit(format!("constraints.{length}.driving")))
    );
    assert_eq!(
        length_parts(&constraint_of(&sketch_of_text(&f, &h, &id), &length).form),
        Some((right, "99".to_owned(), false))
    );
    assert_eq!(never_waits::waits(), 0);
}

/// A 40 × 20 rectangle from (10, 10) to (50, 30), its two horizontals and two verticals:
/// four freedoms, its place and its size, which the counting solver reads as every member
/// under defined.
const BOXED: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Box", "plane": "xy", "points": [{"id": "pa000001", "x": 10, "y": 10, "construction": false}, {"id": "pa000002", "x": 50, "y": 10, "construction": false}, {"id": "pa000003", "x": 50, "y": 30, "construction": false}, {"id": "pa000004", "x": 10, "y": 30, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}]}
  ]
}
"#;

/// The boxed sketch opened in sketch mode (Edit Sketch from the browser), and read.
fn boxed(f: &Fixture) -> AppHarness {
    let (mut h, _) = f.opened("boxed.silico", BOXED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    h
}

/// Sketch mode's drag.
fn the_drag(h: &AppHarness) -> Option<&silico_app::sketch::drag::Drag> {
    h.state().tabs()[0].drag.as_ref()
}

/// The place of the point `id` in what the drag shows.
fn shown_at(h: &AppHarness, id: &str) -> Option<[f64; 2]> {
    let shown = the_drag(h)?.shown()?;
    let p = shown.members.points.iter().find(|p| p.id.as_str() == id)?;
    Some([p.x.mm(), p.y.mm()])
}

fn moved(x: f64, y: f64) -> Vec<silico_app::sketch::PointerEvent> {
    vec![pointer(silico_app::sketch::PointerAction::Move, x, y)]
}

fn drag_sends(h: &AppHarness, what: &'static str) -> usize {
    h.state().tabs()[0]
        .sent
        .iter()
        .filter(|sent| **sent == Sent::SketchEdit(what))
        .count()
}

#[test]
fn a_dragged_corner_follows_the_newest_cursor_one_move_in_flight_the_ui_never_waiting_and_ends_in_one_edit()
 {
    use silico_app::sketch::PointerAction::{Press, Release};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    let text = text_of(&f, &h);
    // Pressed on the corner pa000003, (50, 30), and moved to (52, 32): a drag, begun.
    sketch_events(
        &f,
        &mut h,
        &[
            pointer(Press, 50.0, 30.0),
            pointer(silico_app::sketch::PointerAction::Move, 52.0, 32.0),
        ],
    );
    assert_eq!(drag_sends(&h, "drag_begin"), 1);
    // Once begun, the newest cursor's move; its frame, as the fake answers one (the cursor
    // plus a thousandth of where the corner began), is drawn: (52.05, 32.03).
    step_until(&mut h, "the first frame", |h| {
        the_drag(h).is_some_and(|d| d.frames == 1)
    });
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
    assert!(near(shown_at(&h, "pa000003").unwrap(), [52.05, 32.03]));
    assert_eq!(shown_at(&h, "pa000001"), Some([10.0, 10.0]));
    // Drawn there, over the begin state's ghost at (50, 30).
    sketch_events(&f, &mut h, &[]);
    let points: Vec<eframe::egui::Pos2> = sketch_mode(&h)
        .unwrap()
        .last_drawn()
        .iter()
        .filter(|d| {
            matches!(d, silico_app::sketch::draw::Drawn::Point { .. })
                && d.of().is_some_and(|i| i.as_str() == "pa000003")
        })
        .flat_map(silico_app::sketch::draw::Drawn::positions)
        .collect();
    // Drawn by the pane, through the renderer's f32 projection: to a thousandth of a point.
    let expected = [
        to_screen([50.0, 30.0]).unwrap(),
        to_screen([52.05, 32.03]).unwrap(),
    ];
    assert_eq!(points.len(), 2, "{points:?}");
    for (got, expected) in points.iter().zip(expected) {
        assert!((*got - expected).length() < 1e-3, "{points:?}");
    }
    // The sketch worker held: one move sent, solving; three more cursors over frames that
    // go on, none sent, and the UI thread never waiting on the held move.
    f.sketches.hold();
    sketch_events(&f, &mut h, &moved(54.0, 34.0));
    assert!(f.sketches.wait_held(WAIT));
    assert_eq!(the_drag(&h).unwrap().moves_sent, 2);
    assert_eq!(the_drag(&h).unwrap().last_sent(), Some([54.0, 34.0]));
    for k in 0..3 {
        let k = f64::from(k);
        sketch_events(&f, &mut h, &moved(55.0 + k, 35.0 + k));
        step(&mut h);
    }
    assert_eq!(the_drag(&h).unwrap().moves_sent, 2);
    assert_eq!(never_waits::waits(), 0);
    // Released: the held move answers, and only the newest cursor, (57, 37), goes next:
    // three moves in all, (55, 35) and (56, 36) never sent.
    f.sketches.release();
    step_until(&mut h, "the newest move sent", |h| {
        the_drag(h).is_some_and(|d| d.moves_sent == 3)
    });
    assert_eq!(the_drag(&h).unwrap().last_sent(), Some([57.0, 37.0]));
    step_until(&mut h, "its frame", |h| {
        the_drag(h).is_some_and(|d| d.frames == 3)
    });
    // The release: `drag_end` at its cursor, one edit, one step for undo.
    sketch_events(&f, &mut h, &[pointer(Release, 57.0, 37.0)]);
    settled(&mut h);
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let corner = sketch
        .points
        .iter()
        .find(|p| p.id.as_str() == "pa000003")
        .unwrap();
    assert!(near([corner.x.mm(), corner.y.mm()], [57.05, 37.03]));
    let at = |id: &str| {
        let p = sketch.points.iter().find(|p| p.id.as_str() == id).unwrap();
        [p.x.mm(), p.y.mm()]
    };
    assert_eq!(at("pa000001"), [10.0, 10.0]);
    step_until(&mut h, "a status of the edit", |h| {
        h.state().tabs()[0]
            .transaction()
            .is_some_and(|t| t.undo == 1)
    });
    // Its one undo gives the text back, byte for byte.
    h.state_mut().tabs_mut()[0].step(&Arc::clone(&f.sessions), true);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_and_a_cancel_end_a_drag_from_its_begin_leaving_the_text_and_a_fully_defined_corner_starts_none()
 {
    use silico_app::sketch::PointerAction::{Cancel, Move, Press, Release};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    let text = text_of(&f, &h);
    for (k, end) in [(1, "esc"), (2, "cancel")] {
        sketch_events(
            &f,
            &mut h,
            &[pointer(Press, 50.0, 30.0), pointer(Move, 60.0, 40.0)],
        );
        step_until(&mut h, "the frame", |h| {
            the_drag(h).is_some_and(|d| d.frames >= 1)
        });
        if end == "esc" {
            h.key_press(eframe::egui::Key::Escape);
            step(&mut h);
        } else {
            sketch_events(&f, &mut h, &[pointer(Cancel, 60.0, 40.0)]);
        }
        settled(&mut h);
        step_until(&mut h, "the drag over", |h| the_drag(h).is_none());
        assert_eq!(drag_sends(&h, "drag_end"), k, "{end}");
        assert_eq!(text_of(&f, &h), text, "{end}");
    }
    // Every member fully defined (the fake's own answer: all fixed): pressed and moved, the
    // corner starts no drag, and its release selects it.
    let f = fixture();
    let mut h = boxed(&f);
    sketch_events(
        &f,
        &mut h,
        &[
            pointer(Press, 50.0, 30.0),
            pointer(Move, 60.0, 40.0),
            pointer(Release, 60.0, 40.0),
        ],
    );
    settled(&mut h);
    assert_eq!(drag_sends(&h, "drag_begin"), 0);
    assert!(the_drag(&h).is_none());
    assert_eq!(
        sketch_mode(&h)
            .unwrap()
            .selected_ids()
            .iter()
            .map(|i| i.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["pa000003"]
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_drag_past_the_window_follows_the_unclamped_cursor_and_one_past_reach_is_said_and_the_drag_goes_on()
 {
    use silico_app::sketch::PointerAction::{Move, Press};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // The pane hands over a cursor far outside the viewport, as it is: (−5000, 99999) on
    // the screen, (−510, −9949.9) on the plane.
    let outside = eframe::egui::pos2(-5000.0, 99999.0);
    sketch_events(
        &f,
        &mut h,
        &[
            pointer(Press, 50.0, 30.0),
            silico_app::sketch::PointerEvent {
                action: Move,
                at: outside,
                shift: false,
                ctrl: false,
            },
        ],
    );
    step_until(&mut h, "the frame", |h| {
        the_drag(h).is_some_and(|d| d.frames == 1)
    });
    let cursor = to_sketch(outside).unwrap();
    assert_eq!(the_drag(&h).unwrap().moves_sent, 1);
    assert_eq!(the_drag(&h).unwrap().last_sent(), Some(cursor));
    let shown = shown_at(&h, "pa000003").unwrap();
    assert!((shown[0] - (cursor[0] + 0.05)).abs() < 1e-9);
    assert!((shown[1] - (cursor[1] + 0.03)).abs() < 1e-9);
    // A cursor past the core's reach, a kilometre: 1e8 points right is 9,999,990 mm. The
    // core refuses it, naming `to`, which is said; the drag goes on, the frame kept.
    let far = eframe::egui::pos2(1e8, 200.0);
    sketch_events(
        &f,
        &mut h,
        &[silico_app::sketch::PointerEvent {
            action: Move,
            at: far,
            shift: false,
            ctrl: false,
        }],
    );
    step_until(&mut h, "the refusal said", |h| {
        h.state().tabs()[0]
            .message
            .as_deref()
            .is_some_and(|m| m.contains("the drag's target's x must be finite and within"))
    });
    assert!(the_drag(&h).unwrap().open());
    assert_eq!(the_drag(&h).unwrap().frames, 1);
    // And the next cursor in reach is sent and drawn.
    sketch_events(&f, &mut h, &moved(60.0, 40.0));
    step_until(&mut h, "its frame", |h| {
        the_drag(h).is_some_and(|d| d.frames == 2)
    });
    let shown = shown_at(&h, "pa000003").unwrap();
    assert!((shown[0] - 60.05).abs() < 1e-9 && (shown[1] - 40.03).abs() < 1e-9);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    assert_eq!(never_waits::waits(), 0);
}

// ---- #368's review: a drag's end and sketch mode's leaving; the queue behind a drag ----

/// Stepped until the drag has drawn `frames` solved frames.
fn drag_frames(h: &mut AppHarness, frames: u64) {
    step_until(h, "the drag's frame", |h| {
        the_drag(h).is_some_and(|d| d.frames == frames)
    });
}

/// The session's `status.drag`.
fn status_drag(f: &Fixture, h: &AppHarness) -> Option<silico_core::command::DragOpen> {
    let id = h.state().tabs()[0].id;
    f.sessions.status(&id).result.unwrap().drag
}

#[test]
fn finish_while_a_released_drags_end_is_held_keeps_its_edit_and_one_undo_takes_it_back() {
    use silico_app::sketch::PointerAction::{Move, Press, Release};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    let text = text_of(&f, &h);
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    drag_frames(&mut h, 1);
    // Released with the sketch worker held: its end, sent, solves held.
    f.sketches.hold();
    sketch_events(&f, &mut h, &[pointer(Release, 52.0, 32.0)]);
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    assert!(f.sketches.wait_held(WAIT));
    // Finish: sketch mode closes, and its close waits for the drag's end.
    {
        let sessions = Arc::clone(&f.sessions);
        let _marked = never_waits::marked();
        h.state_mut().tabs_mut()[0].finish_sketch(&sessions, true);
    }
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(sketch_mode(&h).is_none());
    assert!(the_drag(&h).is_some_and(|d| d.in_flight()));
    assert!(h.state().tabs()[0].unsettled());
    f.sketches.release();
    settled(&mut h);
    // The drag's edit stays: the corner at (52.05, 32.03), the fake's frame.
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let corner = sketch
        .points
        .iter()
        .find(|p| p.id.as_str() == "pa000003")
        .unwrap();
    assert!((corner.x.mm() - 52.05).abs() < 1e-9 && (corner.y.mm() - 32.03).abs() < 1e-9);
    assert!(the_drag(&h).is_none());
    assert_eq!(h.state().tabs()[0].message, None);
    // One undo takes back the sketch mode's edits, the drag's: the text byte for byte.
    assert!(h.state().tabs()[0].undoable());
    h.state_mut().tabs_mut()[0].step(&Arc::clone(&f.sessions), true);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn finish_by_its_key_mid_drag_ends_the_drag_from_its_begin_and_leaves_no_drag_open() {
    use silico_app::sketch::PointerAction::{Move, Press};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    let text = text_of(&f, &h);
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    drag_frames(&mut h, 1);
    assert!(status_drag(&f, &h).is_some());
    // ⌘↵ with the press still held.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Enter);
    step(&mut h);
    settled(&mut h);
    assert!(sketch_mode(&h).is_none());
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    assert!(the_drag(&h).is_none());
    assert_eq!(status_drag(&f, &h), None);
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn cancel_after_a_released_drag_whose_end_is_held_undoes_it_with_no_conflict() {
    use silico_app::sketch::PointerAction::{Move, Press, Release};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // A first drag, Finished: a transaction open before sketch mode opens again, so its
    // Cancel is an undo to where it opened, made against the session's revision.
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    drag_frames(&mut h, 1);
    sketch_events(&f, &mut h, &[pointer(Release, 52.0, 32.0)]);
    settled(&mut h);
    step_until(&mut h, "the drag's read", |h| the_drag(h).is_none());
    leave_sketch(&mut h, true);
    let text = text_of(&f, &h);
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    settled(&mut h);
    // A second, of the opposite corner, released with its end held, then Cancel.
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 10.0, 10.0), pointer(Move, 8.0, 8.0)],
    );
    drag_frames(&mut h, 1);
    f.sketches.hold();
    sketch_events(&f, &mut h, &[pointer(Release, 8.0, 8.0)]);
    assert!(f.sketches.wait_held(WAIT));
    {
        let sessions = Arc::clone(&f.sessions);
        let _marked = never_waits::marked();
        h.state_mut().tabs_mut()[0].finish_sketch(&sessions, false);
    }
    for _ in 0..5 {
        step(&mut h);
    }
    f.sketches.release();
    settled(&mut h);
    // Undone to where it opened, the first drag's edit kept, no conflict said.
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(text_of(&f, &h), text);
    assert!(h.state().tabs()[0].transaction);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn delete_during_a_drag_waits_for_its_end_then_goes_against_its_revision_and_both_land() {
    use silico_app::sketch::PointerAction::{Move, Press, Release};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // The bottom, na000001, selected; then the corner pa000003 dragged.
    sketch_events(&f, &mut h, &clicked(30.0, 10.0));
    assert_eq!(
        sketch_mode(&h)
            .unwrap()
            .selected_ids()
            .iter()
            .map(|i| i.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["na000001"]
    );
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    drag_frames(&mut h, 1);
    // A move held in the worker, and Delete pressed: nothing sent while the drag lasts.
    f.sketches.hold();
    sketch_events(&f, &mut h, &moved(54.0, 34.0));
    assert!(f.sketches.wait_held(WAIT));
    h.key_press(eframe::egui::Key::Delete);
    step(&mut h);
    sketch_events(&f, &mut h, &[pointer(Release, 54.0, 34.0)]);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(drag_sends(&h, "sketch_remove"), 0);
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    // The worker let go: the drag ends, then the removal goes, against the drag's revision.
    f.sketches.release();
    settled(&mut h);
    let sent = &h.state().tabs()[0].sent;
    let end = sent
        .iter()
        .position(|s| *s == Sent::SketchEdit("drag_end"))
        .unwrap();
    let remove = sent
        .iter()
        .position(|s| *s == Sent::SketchEdit("sketch_remove"))
        .unwrap();
    assert!(end < remove);
    assert_eq!(h.state().tabs()[0].message, None);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    assert!(sketch.lines.iter().all(|l| l.id.as_str() != "na000001"));
    let corner = sketch
        .points
        .iter()
        .find(|p| p.id.as_str() == "pa000003")
        .unwrap();
    assert!((corner.x.mm() - 54.05).abs() < 1e-9 && (corner.y.mm() - 34.03).abs() < 1e-9);
    assert_eq!(never_waits::waits(), 0);
}

/// [`BOXED`] with the bottom's length, 40.
const BOXED_LENGTH: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Box", "plane": "xy", "points": [{"id": "pa000001", "x": 10, "y": 10, "construction": false}, {"id": "pa000002", "x": 50, "y": 10, "construction": false}, {"id": "pa000003", "x": 50, "y": 30, "construction": false}, {"id": "pa000004", "x": 10, "y": 30, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "length", "line": "na000001", "value": 40, "driving": true}]}
  ]
}
"#;

/// As [`counting`], but a length of 99 mm repeats the others (redundant), which leaves the
/// sketch's constraints agreeing, so a drag may begin; and one of 98 mm disagrees with them
/// (in conflict).
fn counting_redundant_99(f: &Fixture) {
    use silico_protocol::{Solved, SolverConstraint, Tag};
    f.sketches.answer(|system| {
        let sent = FakeSketches::as_sent(system);
        let equations = system
            .constraints
            .iter()
            .filter(|c| c.tag().is_some())
            .count();
        let dofs = system.unknowns.len() as i32 - equations as i32;
        let redundant: Vec<Tag> = system
            .constraints
            .iter()
            .filter_map(|c| match c {
                SolverConstraint::Distance { tag, value, .. }
                    if system.values.get(value.0 as usize) == Some(&99.0) =>
                {
                    Some(*tag)
                }
                _ => None,
            })
            .collect();
        let conflicting: Vec<Tag> = system
            .constraints
            .iter()
            .filter_map(|c| match c {
                SolverConstraint::Distance { tag, value, .. }
                    if system.values.get(value.0 as usize) == Some(&98.0) =>
                {
                    Some(*tag)
                }
                _ => None,
            })
            .collect();
        Ok(Solved {
            dofs,
            redundant,
            conflicting,
            free: if dofs > 0 {
                system.unknowns.clone()
            } else {
                Vec::new()
            },
            ..sent
        })
    });
}

#[test]
fn esc_takes_a_held_drag_before_the_over_define_prompt_and_then_the_prompts_cancel() {
    use silico_app::sketch::PointerAction::{Move, Press};
    use silico_app::sketch::dimension::ModifyAction;
    let f = fixture();
    counting_redundant_99(&f);
    let (mut h, _) = f.opened("boxed.silico", BOXED_LENGTH);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    let text = text_of(&f, &h);
    // The bottom's length set to 99, which repeats the others: the three offered.
    sketch_events(&f, &mut h, &[]);
    let at = label_of_dimension(&h, "ka000005");
    sketch_events(&f, &mut h, &double_clicked_at(at));
    modify(&f, &mut h, ModifyAction::Typed("99".to_owned()));
    modify(&f, &mut h, ModifyAction::Accept);
    settled(&mut h);
    step_until(&mut h, "the offer", |h| {
        sketch_mode(h).is_some_and(|m| m.offer().is_some())
    });
    // The far corner, still free, dragged while the prompt is up.
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    drag_frames(&mut h, 1);
    // The first Esc ends the drag from its begin; the prompt stays.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    assert!(sketch_mode(&h).unwrap().offer().is_some());
    assert!(!h.state().tabs()[0].sent.contains(&Sent::Undo(None)));
    // The second is the prompt's Cancel: the edit undone, the text as it was.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    assert!(sketch_mode(&h).unwrap().offer().is_none());
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Undo(None)));
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_press_on_a_read_older_than_the_sessions_state_begins_no_drag() {
    use silico_app::sketch::PointerAction::{Move, Press};
    use silico_core::document::FeatureId;
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    settled(&mut h);
    // Another client renames the sketch; the tab polled once, which learns the new
    // revision and asks its read, still to be taken: the read drawn is the one before.
    let session = h.state().tabs()[0].id;
    let revision = f.sessions.status(&session).session.unwrap().revision;
    let renamed = f
        .sessions
        .set_field(
            &session,
            revision,
            FeatureId::new("s0000001").unwrap(),
            "name".to_owned(),
            json::read(br#""Renamed""#).unwrap(),
        )
        .wait();
    assert!(renamed.result.is_ok(), "{:?}", renamed.result);
    {
        let sessions = Arc::clone(&f.sessions);
        let _marked = never_waits::marked();
        h.state_mut().tabs_mut()[0].poll(&sessions);
    }
    assert!(h.state().tabs()[0].revision > revision);
    assert_eq!(
        sketch_mode(&h).unwrap().read.as_ref().and_then(|r| r.model),
        Some(revision)
    );
    // A press and a move on it: no drag begun, the grab let go.
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(drag_sends(&h, "drag_begin"), 0);
    assert!(the_drag(&h).is_none());
    // Once the read of the new revision is taken, a press drags again.
    sketch_events(
        &f,
        &mut h,
        &[pointer(
            silico_app::sketch::PointerAction::Release,
            52.0,
            32.0,
        )],
    );
    step_until(&mut h, "the new read", |h| {
        sketch_mode(h)
            .and_then(|m| m.read.as_ref())
            .and_then(|r| r.model)
            .is_some_and(|model| model > revision)
    });
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    assert_eq!(drag_sends(&h, "drag_begin"), 1);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_while_the_begin_is_unanswered_ends_the_drag_once_it_answers() {
    use silico_app::sketch::PointerAction::{Move, Press};
    use silico_core::command::CheckModel;
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    let text = text_of(&f, &h);
    // Another client's fresh check, its rebuild's solve held, ahead in the session's queue.
    f.sketches.hold();
    let session = h.state().tabs()[0].id;
    let check = f
        .sessions
        .check(&session, CheckModel::Current { fresh: true });
    assert!(f.sketches.wait_held(WAIT));
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    assert_eq!(drag_sends(&h, "drag_begin"), 1);
    // Esc with the begin queued behind it: ended, its end to go once the begin answers.
    h.key_press(eframe::egui::Key::Escape);
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(drag_sends(&h, "drag_end"), 0);
    assert!(the_drag(&h).is_some_and(|d| !d.held() && d.in_flight()));
    f.sketches.release();
    assert!(check.wait().result.is_ok());
    settled(&mut h);
    step_until(&mut h, "the drag over", |h| the_drag(h).is_none());
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    assert_eq!(status_drag(&f, &h), None);
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

/// A circle of radius 10 about (30, 30), free.
const CIRCLED: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Round", "plane": "xy", "points": [{"id": "pc000001", "x": 30, "y": 30, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pc000001", "radius": 10, "construction": false}], "arcs": [], "constraints": []}
  ]
}
"#;

#[test]
fn a_line_dragged_by_its_edge_moves_whole_and_a_circles_edge_passes_the_cores_press_check() {
    use silico_app::sketch::PointerAction::{Move, Press, Release};
    use silico_core::command::DragGrab;
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // The bottom, na000001, pressed at (30, 10.3), 0.3 mm off it: grabbed at (30, 10), on
    // it exactly, which the core's press check takes.
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 30.0, 10.3), pointer(Move, 30.0, 15.0)],
    );
    assert_eq!(
        the_drag(&h).unwrap().grab,
        DragGrab::Edge {
            curve: silico_core::document::ElementId::new("na000001").unwrap(),
            press: [30.0, 10.0]
        }
    );
    drag_frames(&mut h, 1);
    // Its two ends by the cursor's offset from the press, (0, 5), each nudged a thousandth
    // of where it began, as the fake answers: (10.01, 15.01) and (50.05, 15.01).
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
    assert!(near(shown_at(&h, "pa000001").unwrap(), [10.01, 15.01]));
    assert!(near(shown_at(&h, "pa000002").unwrap(), [50.05, 15.01]));
    sketch_events(&f, &mut h, &[pointer(Release, 30.0, 15.0)]);
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].message, None);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let at = |id: &str| {
        let p = sketch.points.iter().find(|p| p.id.as_str() == id).unwrap();
        [p.x.mm(), p.y.mm()]
    };
    assert!(near(at("pa000001"), [10.01, 15.01]));
    assert!(near(at("pa000002"), [50.05, 15.01]));
    // A circle's edge: pressed on it at (40, 30), dragged out, begun and ended with no
    // refusal (the fake's frame keeps the radius).
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("circled.silico", CIRCLED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Round"));
    double_click(&mut h, "Browser", "Round");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 40.0, 30.0), pointer(Move, 45.0, 30.0)],
    );
    assert_eq!(
        the_drag(&h).unwrap().grab,
        DragGrab::Edge {
            curve: silico_core::document::ElementId::new("ca000001").unwrap(),
            press: [40.0, 30.0]
        }
    );
    drag_frames(&mut h, 1);
    sketch_events(&f, &mut h, &[pointer(Release, 45.0, 30.0)]);
    settled(&mut h);
    step_until(&mut h, "the drag over", |h| the_drag(h).is_none());
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    assert_eq!(status_drag(&f, &h), None);
    assert_eq!(never_waits::waits(), 0);
}

/// The boxed sketch with its bottom's length, ka000005, opened in sketch mode on `f`.
fn boxed_length(f: &Fixture) -> AppHarness {
    let (mut h, _) = f.opened("boxed.silico", BOXED_LENGTH);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    settled(&mut h);
    h
}

/// The bottom's length, ka000005, double-clicked and set to `text`: stepped until its
/// edit has answered.
fn edit_length(f: &Fixture, h: &mut AppHarness, text: &str) {
    use silico_app::sketch::dimension::ModifyAction;
    sketch_events(f, h, &[]);
    let at = label_of_dimension(h, "ka000005");
    sketch_events(f, h, &double_clicked_at(at));
    assert!(sketch_mode(h).unwrap().edit_box.is_some());
    modify(f, h, ModifyAction::Typed(text.to_owned()));
    modify(f, h, ModifyAction::Accept);
    settled(h);
}

#[test]
fn a_dimension_edited_to_0_is_sent_and_the_gates_refusal_said_in_its_box() {
    let f = fixture();
    counting(&f);
    let mut h = boxed_length(&f);
    let text = text_of(&f, &h);
    edit_length(&f, &mut h, "0");
    // Sent, as the core reads the text (a number it holds); refused by the gate, its words
    // in the box, which stays open, and the text as it was.
    assert!(
        h.state().tabs()[0]
            .sent
            .contains(&Sent::Edit("constraints.ka000005.value".to_owned()))
    );
    let edit = sketch_mode(&h).unwrap().edit_box.clone().unwrap();
    let said = edit.said.unwrap();
    assert_eq!(Some(said.clone()), h.state().tabs()[0].message);
    assert!(!edit.sent);
    assert_eq!(text_of(&f, &h), text);
    assert!(prompt(&h).is_none());
    assert_eq!(never_waits::waits(), 0);
}

/// As [`counting`], but the bottom's length repeats the others while it's 40, the bottom's
/// length as drawn (redundant, agreeing, so the solve stands), and disagrees with them at 98
/// (in conflict, the solve refused).
fn counting_redundant_40(f: &Fixture) {
    use silico_protocol::{Solved, SolverConstraint, Tag};
    f.sketches.answer(|system| {
        let sent = FakeSketches::as_sent(system);
        let equations = system
            .constraints
            .iter()
            .filter(|c| c.tag().is_some())
            .count();
        let dofs = system.unknowns.len() as i32 - equations as i32;
        let valued = |v: f64| -> Vec<Tag> {
            system
                .constraints
                .iter()
                .filter_map(|c| match c {
                    SolverConstraint::Distance { tag, value, .. }
                        if system.values.get(value.0 as usize) == Some(&v) =>
                    {
                        Some(*tag)
                    }
                    _ => None,
                })
                .collect()
        };
        Ok(Solved {
            dofs,
            redundant: valued(40.0),
            conflicting: valued(98.0),
            free: if dofs > 0 {
                system.unknowns.clone()
            } else {
                Vec::new()
            },
            ..sent
        })
    });
}

#[test]
fn an_edit_taking_a_solved_sketch_to_a_refused_solve_offers_though_it_flags_nothing_new() {
    use silico_core::sketch_states::ConstraintState;
    let f = fixture();
    counting_redundant_40(&f);
    let mut h = boxed_length(&f);
    let length = silico_core::document::ElementId::new("ka000005").unwrap();
    // As opened: the bottom's 40 repeats the others, agreeing; the solve stands.
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    assert_eq!(
        read.states.constraints.get(&length),
        Some(&ConstraintState::Redundant)
    );
    assert!(read.states.solve_problem.is_none());
    let state = read.states.state;
    // 98: the same constraint flagged, now in conflict, the solve refused: none newly
    // flagged, the sketch in the state it was, yet offered, for the solve it lost.
    edit_length(&f, &mut h, "98");
    step_until(&mut h, "the read of the edit", |h| {
        sketch_mode(h)
            .and_then(|m| m.drawable())
            .is_some_and(|r| r.states.solve_problem.is_some())
    });
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    assert_eq!(
        read.states.constraints.get(&length),
        Some(&ConstraintState::Conflicting)
    );
    assert_eq!(read.states.state, state);
    step_until(&mut h, "the offer", |h| prompt(h).is_some());
    assert_eq!(
        sketch_mode(&h)
            .unwrap()
            .offer()
            .map(|o| o.constraint.clone()),
        Some(length)
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_right_click_in_sketch_mode_opens_its_menu_whose_cancel_sketch_leaves_it() {
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    let text = text_of(&f, &h);
    let at = to_screen([20.0, 20.0]).unwrap();
    sketch_events(
        &f,
        &mut h,
        &[silico_app::sketch::PointerEvent {
            action: silico_app::sketch::PointerAction::RightClick,
            at,
            shift: false,
            ctrl: false,
        }],
    );
    step_until(&mut h, "the menu", |h| {
        h.query_by_role_and_label(Role::Menu, "Sketch menu")
            .is_some()
    });
    let menu = h.get_by_role_and_label(Role::Menu, "Sketch menu");
    for item in ["End chain", "Finish Sketch", "Cancel sketch"] {
        assert!(
            menu.query_by_role_and_label(Role::Button, item).is_some(),
            "{item}"
        );
    }
    // Where the click was: its first item just inside its frame, from the click.
    let first = menu.get_by_role_and_label(Role::Button, "End chain").rect();
    let off = first.left_top() - at;
    assert!(
        (0.0..16.0).contains(&off.x) && (0.0..16.0).contains(&off.y),
        "{first:?} from {at:?}"
    );
    // No gesture under way: End chain is off.
    assert!(
        menu.get_by_role_and_label(Role::Button, "End chain")
            .accesskit_node()
            .is_disabled()
    );
    menu.get_by_role_and_label(Role::Button, "Cancel sketch")
        .click();
    step(&mut h);
    settled(&mut h);
    assert!(sketch_mode(&h).is_none());
    assert!(
        h.query_by_role_and_label(Role::Menu, "Sketch menu")
            .is_none()
    );
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

// ---- The lean review of 453ccfd8 and f89d73c7 ----

/// As [`counting`], but a length of 97 mm or 98 mm disagrees with the others (in conflict,
/// the solve refused), whichever it is.
fn counting_conflicting_97_98(f: &Fixture) {
    use silico_protocol::{Solved, SolverConstraint, Tag};
    f.sketches.answer(|system| {
        let sent = FakeSketches::as_sent(system);
        let equations = system
            .constraints
            .iter()
            .filter(|c| c.tag().is_some())
            .count();
        let dofs = system.unknowns.len() as i32 - equations as i32;
        let conflicting: Vec<Tag> = system
            .constraints
            .iter()
            .filter_map(|c| match c {
                SolverConstraint::Distance { tag, value, .. }
                    if matches!(system.values.get(value.0 as usize), Some(&(97.0 | 98.0))) =>
                {
                    Some(*tag)
                }
                _ => None,
            })
            .collect();
        Ok(Solved {
            dofs,
            conflicting,
            free: if dofs > 0 {
                system.unknowns.clone()
            } else {
                Vec::new()
            },
            ..sent
        })
    });
}

#[test]
fn an_edit_on_a_sketch_already_refused_and_still_so_offers_nothing() {
    use silico_core::sketch_states::ConstraintState;
    let f = fixture();
    counting_conflicting_97_98(&f);
    // The bottom's length at 98, in conflict as opened: the solve refused already.
    let (mut h, _) = f.opened(
        "boxed.silico",
        &BOXED_LENGTH.replace(r#""value": 40"#, r#""value": 98"#),
    );
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    settled(&mut h);
    let length = silico_core::document::ElementId::new("ka000005").unwrap();
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    assert_eq!(
        read.states.constraints.get(&length),
        Some(&ConstraintState::Conflicting)
    );
    assert!(read.states.solve_problem.is_some());
    let state = read.states.state;
    // 98 to 97: the same constraint in conflict, the solve refused before and after, the
    // state the same. Nothing for the edit to answer for: no prompt once its read lands.
    edit_length(&f, &mut h, "97");
    step_until(&mut h, "the edit's read judged", |h| {
        sketch_mode(h).is_some_and(|m| {
            m.edited.is_none()
                && m.drawable().is_some_and(|r| {
                    r.members.constraints.iter().any(|c| {
                        c.id.as_str() == "ka000005" && c.form.value_text().as_deref() == Some("97")
                    })
                })
        })
    });
    for _ in 0..10 {
        step(&mut h);
    }
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    assert_eq!(
        read.states.constraints.get(&length),
        Some(&ConstraintState::Conflicting)
    );
    assert!(read.states.solve_problem.is_some());
    assert_eq!(read.states.state, state);
    assert!(sketch_mode(&h).unwrap().offer().is_none());
    assert!(prompt(&h).is_none());
    assert_eq!(never_waits::waits(), 0);
}

/// Sketch mode's canvas menu, if it's shown.
fn sketch_menu(h: &AppHarness) -> Option<egui_kittest::Node<'_>> {
    h.query_by_role_and_label(Role::Menu, "Sketch menu")
}

/// A right-click at (`x`, `y`) on the plane, as the pane hands one over, and the menu shown.
fn open_menu(f: &Fixture, h: &mut AppHarness, x: f64, y: f64) {
    let event = pointer(silico_app::sketch::PointerAction::RightClick, x, y);
    sketch_events(f, h, &[event]);
    step_until(h, "the menu", |h| sketch_menu(h).is_some());
    assert!(sketch_mode(h).unwrap().menu_at.is_some());
}

/// The left button pressed at `from` (screen points) and let go at `to`, a frame apart, as
/// egui sees the window's pointer.
fn press_away(h: &mut AppHarness, from: eframe::egui::Pos2, to: eframe::egui::Pos2) {
    use eframe::egui::{Event, Modifiers, PointerButton};
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
    });
    step(h);
    h.input_mut().events.push(Event::PointerMoved(to));
    step(h);
    h.input_mut().events.push(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
    });
    step(h);
}

#[test]
fn the_canvas_menu_closes_on_esc_on_a_click_or_a_drag_away_from_it_and_on_a_tool_chosen() {
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    let canvas = the_viewport(&h);
    // Esc closes it; sketch mode stays.
    open_menu(&f, &mut h, 20.0, 20.0);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(sketch_menu(&h).is_none());
    assert!(sketch_mode(&h).is_some_and(|m| m.menu_at.is_none()));
    // A left click on the canvas away from it closes it; sketch mode stays.
    open_menu(&f, &mut h, 20.0, 20.0);
    let away = canvas.right_bottom() - eframe::egui::vec2(60.0, 60.0);
    press_away(&mut h, away, away);
    assert!(sketch_menu(&h).is_none());
    assert!(sketch_mode(&h).is_some_and(|m| m.menu_at.is_none()));
    // So does a drag begun away from it, at its press.
    open_menu(&f, &mut h, 20.0, 20.0);
    press_away(&mut h, away, away - eframe::egui::vec2(100.0, 100.0));
    assert!(sketch_menu(&h).is_none());
    assert!(sketch_mode(&h).is_some_and(|m| m.menu_at.is_none()));
    // A tool chosen by its key closes it, the tool on.
    open_menu(&f, &mut h, 20.0, 20.0);
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    step(&mut h);
    assert!(sketch_mode(&h).is_some_and(|m| m.menu_at.is_none()));
    assert!(sketch_menu(&h).is_none());
    assert_eq!(
        sketch_mode(&h).unwrap().tools.choice(),
        Some(silico_app::sketch::tools::Choice::Draw(
            silico_core::sketch_tools::ToolKind::Line
        ))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_canvas_menus_end_chain_ends_a_chain_under_way_closes_the_menu_and_keeps_sketch_mode() {
    use silico_app::sketch::tools::Choice;
    use silico_core::sketch_tools::ToolKind;
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // A line's chain begun: its first click, at (70, 70), away from the box.
    h.key_press(eframe::egui::Key::L);
    step(&mut h);
    sketch_events(&f, &mut h, &clicked(70.0, 70.0));
    assert!(sketch_mode(&h).unwrap().tools.busy());
    open_menu(&f, &mut h, 80.0, 60.0);
    let end = sketch_menu(&h)
        .unwrap()
        .get_by_role_and_label(Role::Button, "End chain")
        .accesskit_node()
        .is_disabled();
    assert!(!end);
    sketch_menu(&h)
        .unwrap()
        .get_by_role_and_label(Role::Button, "End chain")
        .click();
    step(&mut h);
    step(&mut h);
    assert!(sketch_menu(&h).is_none());
    let mode = sketch_mode(&h).unwrap();
    assert!(mode.menu_at.is_none());
    assert!(!mode.tools.busy());
    assert_eq!(mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    assert_eq!(never_waits::waits(), 0);
}

// ---- Sketch mode through the pane (M5.11's wiring: `Tab::overlay`) ----

/// Where the top views below centre the image: (40, 20) on XY.
const TOP_TARGET: [f64; 2] = [40.0, 20.0];
/// Their height, in millimetres.
const TOP_HEIGHT: f64 = 100.0;

/// The viewport's camera straight down on XY, orthographic, [`TOP_HEIGHT`] high, its
/// image's centre over [`TOP_TARGET`], x right and y up the image; and frames until the
/// pane's frame has it.
fn top_view(h: &mut AppHarness) {
    let [x, y] = TOP_TARGET;
    let camera = silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic {
            view_height: TOP_HEIGHT,
        },
        eye: [x, y, 1000.0],
        target: [x, y, 0.0],
        up: [0.0, 1.0, 0.0],
    };
    view_mut(h).set_camera(camera.clone());
    step_until(h, "the top view drawn", |h| {
        view(h).frame().is_some_and(|frame| frame.camera == camera)
    });
}

/// Where [`top_view`] shows XY's place `at`, worked by hand from the pane's frame: the
/// image's `size_px` high over [`TOP_HEIGHT`], its centre over [`TOP_TARGET`], then from
/// physical pixels to points from the viewport's top left.
fn top_screen(h: &AppHarness, at: [f64; 2]) -> eframe::egui::Pos2 {
    let frame = view(h).frame().unwrap();
    let ppp = f64::from(frame.pixels_per_point);
    let [width, height] = frame.size_px.map(f64::from);
    let mm = TOP_HEIGHT / height;
    let pixel = [
        width / 2.0 + (at[0] - TOP_TARGET[0]) / mm,
        height / 2.0 - (at[1] - TOP_TARGET[1]) / mm,
    ];
    eframe::egui::pos2(
        (f64::from(frame.rect.left()) + pixel[0] / ppp) as f32,
        (f64::from(frame.rect.top()) + pixel[1] / ppp) as f32,
    )
}

/// The left button pressed at XY's place `at` in [`top_view`], on the screen, as the
/// window's pointer gives it; a frame.
fn pane_press(h: &mut AppHarness, at: [f64; 2]) {
    let at = top_screen(h, at);
    let none = eframe::egui::Modifiers::NONE;
    h.hover_at(at);
    button_event(h, at, eframe::egui::PointerButton::Primary, true, none);
    step(h);
}

/// The pointer moved to `at`; a frame.
fn pane_move(h: &mut AppHarness, at: [f64; 2]) {
    let at = top_screen(h, at);
    h.hover_at(at);
    step(h);
}

/// The left button let go at `at`; a frame.
fn pane_release(h: &mut AppHarness, at: [f64; 2]) {
    let at = top_screen(h, at);
    let none = eframe::egui::Modifiers::NONE;
    h.hover_at(at);
    button_event(h, at, eframe::egui::PointerButton::Primary, false, none);
    step(h);
}

/// A click at `at`: pressed and let go a frame apart.
fn pane_click(h: &mut AppHarness, at: [f64; 2]) {
    pane_press(h, at);
    pane_release(h, at);
}

/// The one point of `sketch` within `within` of (x, y), its ID.
fn point_near(sketch: &silico_core::document::Sketch, x: f64, y: f64, within: f64) -> String {
    let found: Vec<&str> = sketch
        .points
        .iter()
        .filter(|p| (p.x.mm() - x).abs() <= within && (p.y.mm() - y).abs() <= within)
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(found.len(), 1, "one point near ({x}, {y}) in {sketch:?}");
    found[0].to_owned()
}

/// What a click through the pane comes to on the plane: the pointer's place in f32
/// points, a ray, and the renderer's f32 projection. A ten-thousandth of a millimetre.
const THROUGH_THE_PANE: f64 = 1e-4;

/// [`assert_rectangle_in`], each corner within [`THROUGH_THE_PANE`] of where it was asked.
fn assert_rectangle_near(sketch: &silico_core::document::Sketch, lo: [f64; 2], hi: [f64; 2]) {
    let corner = |x, y| point_near(sketch, x, y, THROUGH_THE_PANE);
    let a = corner(lo[0], lo[1]);
    let b = corner(hi[0], lo[1]);
    let c = corner(hi[0], hi[1]);
    let d = corner(lo[0], hi[1]);
    for (from, to, horizontal) in [
        (&a, &b, true),
        (&b, &c, false),
        (&c, &d, true),
        (&d, &a, false),
    ] {
        let side = line_between(sketch, from, to).unwrap_or_else(|| panic!("{from} to {to}"));
        assert!(holds_direction(sketch, &side, horizontal), "{side}");
        assert!(!holds_direction(sketch, &side, !horizontal), "{side}");
    }
}

#[test]
fn a_rectangle_dragged_through_the_panes_left_events_at_two_pixels_a_point_is_four_lines_two_level_two_plumb()
 {
    use silico_app::sketch::tools::Choice;
    use silico_core::sketch_tools::ToolKind;
    let f = fixture();
    let (mut h, _) = f.overlaid(EXAMPLE);
    assert_eq!(h.ctx.pixels_per_point(), 2.0);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create_sketch(&mut h, "XY plane");
    sketch_read(&mut h);
    let id = sketch_mode(&h).unwrap().sketch.clone().unwrap();
    top_view(&mut h);
    h.key_press(eframe::egui::Key::R);
    step(&mut h);
    assert_eq!(
        sketch_mode(&h).unwrap().tools.choice(),
        Some(Choice::Draw(ToolKind::CornerRectangle))
    );
    // Pressed at (25, 12), dragged through (40, 20), let go at (55, 28): the pane's own
    // events, at its physical pixels, through `Tab::overlay`, and nothing by hand.
    let (from, to) = ([25.0, 12.0], [55.0, 28.0]);
    pane_press(&mut h, from);
    pane_move(&mut h, [40.0, 20.0]);
    pane_move(&mut h, to);
    // The pointer read on the plane, for the status bar.
    let pointer = sketch_mode(&h).unwrap().pointer.unwrap();
    assert!((pointer[0] - to[0]).abs() < THROUGH_THE_PANE, "{pointer:?}");
    assert!((pointer[1] - to[1]).abs() < THROUGH_THE_PANE, "{pointer:?}");
    pane_release(&mut h, to);
    settled(&mut h);
    assert_eq!(adds_sent(&h), 1);
    let sketch = sketch_of_text(&f, &h, id.as_str());
    assert_eq!((sketch.points.len(), sketch.lines.len()), (4, 4));
    assert_eq!(sketch.constraints.len(), 4, "{:?}", sketch.constraints);
    assert_rectangle_near(&sketch, from, to);
    // And it reads back so: four lines, two horizontals and two verticals.
    read_with_lines(&mut h, 4);
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    let directions = |level: bool| {
        read.members
            .lines
            .iter()
            .filter(|line| holds_direction(&read.members, line.id.as_str(), level))
            .count()
    };
    assert_eq!((directions(true), directions(false)), (2, 2));
    // Drawn where the renderer would draw it: each corner's point at its place on the screen
    // (a thousandth of a point).
    step(&mut h);
    for corner in [from, [to[0], from[1]], to, [from[0], to[1]]] {
        let point = point_near(&read.members, corner[0], corner[1], THROUGH_THE_PANE);
        let drawn: Vec<eframe::egui::Pos2> = sketch_mode(&h)
            .unwrap()
            .last_drawn()
            .iter()
            .filter(|d| {
                matches!(d, silico_app::sketch::draw::Drawn::Point { .. })
                    && d.of().is_some_and(|i| i.as_str() == point)
            })
            .flat_map(silico_app::sketch::draw::Drawn::positions)
            .collect();
        assert_eq!(drawn.len(), 1, "{drawn:?}");
        let expected = top_screen(&h, corner);
        assert!(
            (drawn[0] - expected).length() < 1e-2,
            "{drawn:?} {expected:?}"
        );
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_dimension_placed_through_the_pane_picks_the_side_clicked_and_opens_its_modify_box_there() {
    use silico_app::sketch::dimension::ModifyAction;
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.overlaid(BOXED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    top_view(&mut h);
    h.key_press(eframe::egui::Key::D);
    step(&mut h);
    // The right side, na000002 from (50, 10) to (50, 30), clicked at its middle; then the
    // place, 10 to its right: its length, 20, in the Modify box at the place.
    pane_click(&mut h, [50.0, 20.0]);
    pane_click(&mut h, [60.0, 20.0]);
    let place = top_screen(&h, [60.0, 20.0]);
    step_until(&mut h, "the Modify box open", |h| {
        sketch_mode(h).and_then(|mode| mode.modify_view()).is_some()
    });
    let view = sketch_mode(&h).unwrap().modify_view().unwrap();
    let measured: f64 = view.text.parse().unwrap();
    assert!((measured - 20.0).abs() < THROUGH_THE_PANE, "{}", view.text);
    assert!(view.placing);
    // Shown where the place was clicked, as the pane projects it (a hundredth of a point).
    assert!((view.at - place).length() < 1e-2, "{:?} {place:?}", view.at);
    modify(&f, &mut h, ModifyAction::Typed("20".to_owned()));
    modify(&f, &mut h, ModifyAction::Accept);
    settled(&mut h);
    read_with_constraints(&mut h, 5);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let lengths = lengths(&sketch);
    assert_eq!(lengths.len(), 1, "{lengths:?}");
    let (_, line, value, driving) = &lengths[0];
    assert_eq!(
        (line.as_str(), value.as_str(), *driving),
        ("na000002", "20", true)
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_corner_dragged_through_the_pane_follows_the_cursor_and_ends_in_one_edit_there() {
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.overlaid(BOXED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    top_view(&mut h);
    let text = text_of(&f, &h);
    // The free corner pa000003, (50, 30), pressed and moved to (52, 32): a drag, begun, its
    // first frame as the fake answers (the cursor plus a thousandth of where it began).
    pane_press(&mut h, [50.0, 30.0]);
    pane_move(&mut h, [52.0, 32.0]);
    assert_eq!(drag_sends(&h, "drag_begin"), 1);
    step_until(&mut h, "the first frame", |h| {
        the_drag(h).is_some_and(|d| d.frames >= 1)
    });
    let shown = shown_at(&h, "pa000003").unwrap();
    assert!((shown[0] - 52.05).abs() < THROUGH_THE_PANE, "{shown:?}");
    assert!((shown[1] - 32.03).abs() < THROUGH_THE_PANE, "{shown:?}");
    // Its ghost, where it began, drawn faintly behind it: the point twice, at both places.
    step(&mut h);
    let drawn: Vec<eframe::egui::Pos2> = sketch_mode(&h)
        .unwrap()
        .last_drawn()
        .iter()
        .filter(|d| {
            matches!(d, silico_app::sketch::draw::Drawn::Point { .. })
                && d.of().is_some_and(|i| i.as_str() == "pa000003")
        })
        .flat_map(silico_app::sketch::draw::Drawn::positions)
        .collect();
    assert_eq!(drawn.len(), 2, "{drawn:?}");
    assert!((drawn[0] - top_screen(&h, [50.0, 30.0])).length() < 1e-2);
    assert!((drawn[1] - top_screen(&h, [52.05, 32.03])).length() < 1e-2);
    // Let go there: one `drag_end`, the corner written where its last frame put it.
    pane_release(&mut h, [52.0, 32.0]);
    settled(&mut h);
    step_until(&mut h, "the drag over", |h| the_drag(h).is_none());
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    point_near(&sketch, 52.05, 32.03, THROUGH_THE_PANE);
    point_near(&sketch, 10.0, 10.0, 0.0);
    assert_ne!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_drag_held_when_its_tab_is_hidden_ends_from_its_begin_and_its_corner_is_where_it_was_on_return()
{
    let f = fixture();
    counting(&f);
    let (mut h, seen) = f.overlaid(BOXED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    top_view(&mut h);
    let text = text_of(&f, &h);
    let first = h.state().tabs()[0].id;
    // The corner pressed and dragged to (60, 40), begun and drawn there, still held.
    pane_press(&mut h, [50.0, 30.0]);
    pane_move(&mut h, [60.0, 40.0]);
    step_until(&mut h, "the first frame", |h| {
        the_drag(h).is_some_and(|d| d.frames >= 1)
    });
    assert!(the_drag(&h).unwrap().held());
    left_taken(&seen);
    // Another document opened over it, the button still held: the first tab's press ends in
    // a Cancel given through its own overlay, which ends the drag from its begin state.
    let other = f.file("other.silico", EXAMPLE);
    h.state_mut().open(other.to_str().unwrap(), false);
    step_until(&mut h, "the second tab", |h| h.state().tabs().len() == 2);
    let events = left_taken(&seen);
    let cancels: Vec<_> = events
        .iter()
        .filter(|e| e.session == first)
        .map(|e| e.event.kind)
        .collect();
    assert_eq!(
        cancels,
        [silico_app::viewport::LeftKind::Cancel],
        "{events:?}"
    );
    step_until(&mut h, "the drag over", |h| the_drag(h).is_none());
    assert_eq!(drag_sends(&h, "drag_end"), 1);
    // Let go and moved in the second tab: nothing of the first's.
    pane_release(&mut h, [62.0, 42.0]);
    pane_move(&mut h, [64.0, 44.0]);
    settled(&mut h);
    assert_eq!(text_of(&f, &h), text);
    // Back on the first: the corner where it was, read and drawn, with no drag.
    h.get_by_role_and_label(Role::Button, "example.silico")
        .click();
    step_until(&mut h, "the first shown", |h| left_shown(h) == Some(first));
    top_view(&mut h);
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(the_drag(&h).is_none());
    let read = sketch_mode(&h).unwrap().drawable().unwrap().clone();
    point_near(&read.members, 50.0, 30.0, 0.0);
    let drawn: Vec<eframe::egui::Pos2> = sketch_mode(&h)
        .unwrap()
        .last_drawn()
        .iter()
        .filter(|d| {
            matches!(d, silico_app::sketch::draw::Drawn::Point { .. })
                && d.of().is_some_and(|i| i.as_str() == "pa000003")
        })
        .flat_map(silico_app::sketch::draw::Drawn::positions)
        .collect();
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert!((drawn[0] - top_screen(&h, [50.0, 30.0])).length() < 1e-2);
    assert_eq!(drag_sends(&h, "drag_begin"), 1);
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}

/// The members sketch mode has selected, by ID.
fn selected(h: &AppHarness) -> Vec<String> {
    sketch_mode(h)
        .unwrap()
        .selected_ids()
        .iter()
        .map(|i| i.as_str().to_owned())
        .collect()
}

#[test]
fn esc_on_a_press_of_a_free_corner_lets_the_press_go_so_its_release_selects_nothing() {
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.overlaid(BOXED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    top_view(&mut h);
    let corner = [50.0, 30.0];
    // Unheld, a click on the free corner selects it: what the Esc below takes away.
    pane_click(&mut h, corner);
    assert_eq!(selected(&h), ["pa000003"]);
    pane_click(&mut h, [30.0, 45.0]);
    assert!(selected(&h).is_empty());
    // Pressed, Esc, let go where it was pressed: a drag's press, let go, selects nothing.
    pane_press(&mut h, corner);
    assert!(the_drag(&h).is_some_and(|d| d.held() && d.shown().is_none()));
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(the_drag(&h).is_none());
    pane_release(&mut h, corner);
    step(&mut h);
    assert!(selected(&h).is_empty(), "{:?}", selected(&h));
    assert_eq!(drag_sends(&h, "drag_begin"), 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_on_an_unmoved_held_press_is_the_presss_alone_the_selection_kept_and_no_drag_after() {
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.overlaid(BOXED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    top_view(&mut h);
    // The bottom side, na000001, selected by a click at its middle.
    pane_click(&mut h, [30.0, 10.0]);
    assert_eq!(selected(&h), ["na000001"]);
    // The free corner pressed, not moved: Esc ends the press, and nothing else (the
    // selection is Esc's next, and it stays).
    let corner = [50.0, 30.0];
    pane_press(&mut h, corner);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(the_drag(&h).is_none());
    assert_eq!(selected(&h), ["na000001"]);
    // Moved past the drag threshold and let go, still the same press: no drag begins, and
    // the release selects nothing.
    pane_move(&mut h, [55.0, 35.0]);
    pane_release(&mut h, [55.0, 35.0]);
    settled(&mut h);
    assert!(the_drag(&h).is_none());
    assert_eq!(drag_sends(&h, "drag_begin"), 0);
    assert_eq!(selected(&h), ["na000001"]);
    // A second Esc clears it, as with no press.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(selected(&h).is_empty());
    assert_eq!(never_waits::waits(), 0);
}

/// The right button pressed and let go at `at`: in one frame (`together`), or a frame apart.
fn right_click_at(h: &mut AppHarness, at: eframe::egui::Pos2, together: bool) {
    let right = eframe::egui::PointerButton::Secondary;
    let none = eframe::egui::Modifiers::NONE;
    h.hover_at(at);
    button_event(h, at, right, true, none);
    if !together {
        step(h);
    }
    button_event(h, at, right, false, none);
    step(h);
}

#[test]
fn a_right_click_on_the_canvas_opens_the_sketch_menu_at_it_and_its_own_release_leaves_it_open() {
    let f = fixture();
    let (mut h, _) = f.overlaid(EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create_sketch(&mut h, "XY plane");
    sketch_read(&mut h);
    top_view(&mut h);
    for together in [true, false] {
        let at = top_screen(&h, [30.0, 15.0]);
        right_click_at(&mut h, at, together);
        // Handed on through `ViewFrame.secondary`, and opened at the click in that frame:
        // its press and release, let go in it, don't close it.
        assert_eq!(sketch_mode(&h).unwrap().menu_at, Some(at), "{together}");
        assert!(sketch_menu(&h).is_some(), "{together}");
        for _ in 0..3 {
            step(&mut h);
        }
        assert!(sketch_menu(&h).is_some(), "{together}");
        // At the click: its first item just below and right of it.
        let item = sketch_menu(&h)
            .unwrap()
            .get_by_role_and_label(Role::Button, "End chain")
            .rect();
        let off = item.min - at;
        assert!(
            (0.0..24.0).contains(&off.x) && (0.0..24.0).contains(&off.y),
            "{item:?} {at:?}"
        );
        // A left click away closes it, and is the canvas's click too.
        pane_click(&mut h, [60.0, 35.0]);
        step(&mut h);
        assert!(sketch_menu(&h).is_none(), "{together}");
        assert_eq!(sketch_mode(&h).unwrap().menu_at, None);
    }
    assert_eq!(never_waits::waits(), 0);
}

// ---- M5.11's wiring fixes: the Modify box clear of the palette, Properties after the
// over-define prompt's Cancel, and the canvas menu at fractional scales ----

/// The Sketch Palette's place on the screen, as last drawn: its area's, the app's
/// `sketch_palette` of the tab shown.
fn palette_rect(h: &AppHarness) -> eframe::egui::Rect {
    let tab = h.state().tabs()[0].id.to_string();
    let id = eframe::egui::Id::new(("sketch_palette", tab));
    eframe::egui::AreaState::load(&h.ctx, id).unwrap().rect()
}

#[test]
fn a_dimension_whose_text_is_under_the_palette_opens_its_modify_box_whole_in_the_viewport_and_clear_of_the_palette()
 {
    use silico_app::sketch::dimension::ModifyAction;
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // The app's fonts and icons, so text and icons have their sizes.
    silico_app::theme::apply(&h.ctx);
    step(&mut h);
    hand_camera(&mut h);
    h.key_press(eframe::egui::Key::D);
    step(&mut h);
    let palette = palette_rect(&h);
    let viewport = view(&h).frame().unwrap().rect;
    // The place, 40 points into the palette, level with the right side's middle (y 20, at
    // 300 on the screen by the hand camera): the side's length, its text under the palette.
    let under = eframe::egui::pos2(palette.left() + 40.0, to_screen([0.0, 20.0]).unwrap().y);
    assert!(
        palette.contains(under) && viewport.contains(under),
        "{under:?} {palette:?}"
    );
    let place = to_sketch(under).unwrap();
    sketch_events(&f, &mut h, &clicked(50.0, 20.0));
    sketch_events(&f, &mut h, &clicked(place[0], place[1]));
    step_until(&mut h, "the Modify box open", |h| {
        sketch_mode(h).and_then(|mode| mode.modify_view()).is_some()
    });
    let shown = sketch_mode(&h).unwrap().modify_view().unwrap();
    assert!(palette.contains(shown.at), "{:?} {palette:?}", shown.at);
    // Sized in its first frame, unseen, then placed.
    for _ in 0..3 {
        step(&mut h);
    }
    let dialog = h.get_by_role_and_label(Role::Dialog, "Modify dimension");
    let area = eframe::egui::Id::new(("sketch_modify", shown.opened));
    let parts = [
        (
            "the box",
            eframe::egui::AreaState::load(&h.ctx, area).unwrap().rect(),
        ),
        (
            "its field",
            dialog.get_by_label("Value in millimetres").rect(),
        ),
        (
            "its ✓",
            dialog.get_by_role_and_label(Role::Button, "Accept").rect(),
        ),
        (
            "its ✕",
            dialog.get_by_role_and_label(Role::Button, "Cancel").rect(),
        ),
        (
            "its hint",
            h.get_by_label("A plain number in mm · Enter accepts")
                .rect(),
        ),
    ];
    // egui puts an area's corner on a pixel: within half of one of the viewport.
    let half = 0.5 / h.ctx.pixels_per_point();
    for (what, rect) in parts {
        assert!(
            !rect.intersects(palette),
            "{what} {rect:?} under {palette:?}"
        );
        assert!(
            rect.right() <= palette.left() - 8.0 + half,
            "{what} {rect:?} not left of {palette:?}"
        );
        assert!(
            viewport.expand(half).contains_rect(rect),
            "{what} {rect:?} outside {viewport:?}"
        );
    }
    // Still the dimension's box: its value, and Enter places it.
    assert_eq!(shown.text, "20");
    modify(&f, &mut h, ModifyAction::Accept);
    settled(&mut h);
    read_with_constraints(&mut h, 5);
    assert_eq!(never_waits::waits(), 0);
}

/// The boxed sketch beside a block with no fingerprint, whose warning marks the timeline
/// whatever the sketch's state.
const BOXED_BESIDE_A_BLOCK: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [-100, -100, 0], "length": 40, "width": 20, "height": 10},
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Box", "plane": "xy", "points": [{"id": "pa000001", "x": 10, "y": 10, "construction": false}, {"id": "pa000002", "x": 50, "y": 10, "construction": false}, {"id": "pa000003", "x": 50, "y": 30, "construction": false}, {"id": "pa000004", "x": 10, "y": 30, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}]}
  ]
}
"#;

/// The problems and warnings the tab's newest timeline marks feature `id` with.
fn marks(h: &AppHarness, id: &str) -> Option<(u64, u64)> {
    h.state().tabs()[0]
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.features.as_ref())
        .and_then(|features| features.iter().find(|e| e.id.as_str() == id))
        .map(|entry| (entry.problems, entry.warnings))
}

#[test]
fn after_the_over_define_prompts_cancel_properties_says_what_the_newest_read_says_not_the_conflict()
{
    use silico_app::sketch::dimension::ModifyAction;
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("boxed.silico", BOXED_BESIDE_A_BLOCK);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    step_until(&mut h, "the sketch in Properties", |h| {
        shows(h, "Properties", "Type: sketch")
    });
    // The block's warning marks the timeline, so a read that marks nothing never clears
    // what was said wholesale.
    assert!(marks(&h, "k7f2qx3m").is_some_and(|(_, warnings)| warnings > 0));
    // The right side's length as 99, which no sketch here takes: over defined, offered.
    h.key_press(eframe::egui::Key::D);
    step(&mut h);
    sketch_events(&f, &mut h, &clicked(50.0, 20.0));
    sketch_events(&f, &mut h, &clicked(60.0, 20.0));
    step_until(&mut h, "the Modify box open", |h| {
        sketch_mode(h).and_then(|mode| mode.modify_view()).is_some()
    });
    modify(&f, &mut h, ModifyAction::Typed("99".to_owned()));
    modify(&f, &mut h, ModifyAction::Accept);
    settled(&mut h);
    step_until(&mut h, "the prompt", |h| prompt(h).is_some());
    step_until(&mut h, "Properties says the conflict", |h| {
        showing(h, "Properties", "conflict").is_some()
    });
    // Cancel (Esc, the prompt's): `undo`, then the read of the revision it made.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Undo(None)));
    step_until(&mut h, "the sketch unmarked by the newest read", |h| {
        marks(h, "s0000001") == Some((0, 0)) && prompt(h).is_none()
    });
    for _ in 0..5 {
        step(&mut h);
    }
    // The block still marked: the timeline marks something, the sketch nothing.
    assert!(marks(&h, "k7f2qx3m").is_some_and(|(_, warnings)| warnings > 0));
    assert!(shows(&h, "Properties", "Type: sketch"));
    assert_eq!(showing(&h, "Properties", "conflict"), None);
    assert!(!h.state().tabs()[0].said.contains_key("s0000001"));
    assert_eq!(never_waits::waits(), 0);
}

/// The right button pressed at `from` and let go at `to` in one frame.
fn right_click_moved(h: &mut AppHarness, from: eframe::egui::Pos2, to: eframe::egui::Pos2) {
    let right = eframe::egui::PointerButton::Secondary;
    let none = eframe::egui::Modifiers::NONE;
    h.hover_at(from);
    button_event(h, from, right, true, none);
    h.event(eframe::egui::Event::PointerMoved(to));
    button_event(h, to, right, false, none);
    step(h);
}

/// The left button pressed and let go at `at`, a frame apart.
fn left_click_at(h: &mut AppHarness, at: eframe::egui::Pos2) {
    let left = eframe::egui::PointerButton::Primary;
    let none = eframe::egui::Modifiers::NONE;
    h.hover_at(at);
    button_event(h, at, left, true, none);
    step(h);
    button_event(h, at, left, false, none);
    step(h);
}

#[test]
fn at_one_and_a_quarter_and_one_and_a_half_pixels_a_point_the_sketch_menu_stays_open_for_its_own_press_and_one_on_its_edge_pixel()
 {
    use eframe::egui::pos2;
    for ppp in [1.25_f32, 1.5] {
        let f = fixture();
        let (mut h, _) = f.overlaid_at(EXAMPLE, ppp);
        assert_eq!(h.ctx.pixels_per_point(), ppp);
        step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
        create_sketch(&mut h, "XY plane");
        sketch_read(&mut h);
        // Pressed at (500.5, 400.5) and let go 3 points right and down, in one frame (a
        // slow frame's click): the press, before the menu's corner, opened it, and leaves
        // it open.
        let (from, to) = (pos2(500.5, 400.5), pos2(503.5, 403.5));
        right_click_moved(&mut h, from, to);
        let opened = sketch_mode(&h).unwrap().menu_at.unwrap();
        assert!((opened - to).length() < 1e-3, "{ppp} {opened:?}");
        for _ in 0..3 {
            step(&mut h);
        }
        assert!(sketch_menu(&h).is_some(), "{ppp}");
        left_click_at(&mut h, pos2(400.0, 300.0));
        assert!(sketch_menu(&h).is_none(), "{ppp}");
        // Opened at (501, 401), a frame apart: its corner is the click's pixel's, which
        // egui's 1/32-point rounding moves off the pixel's edge in points (at 1.25, past the
        // press; at 1.5, before it). Pixels worked by hand: 626 and 501 at 1.25, 751 and
        // 601 at 1.5.
        let at = pos2(501.0, 401.0);
        let right = eframe::egui::PointerButton::Secondary;
        let none = eframe::egui::Modifiers::NONE;
        h.hover_at(at);
        button_event(&h, at, right, true, none);
        step(&mut h);
        button_event(&h, at, right, false, none);
        step(&mut h);
        step(&mut h);
        assert!(sketch_menu(&h).is_some(), "{ppp}");
        let [x, y]: [f32; 2] = if ppp == 1.25 {
            [626.0, 501.0]
        } else {
            [751.0, 601.0]
        };
        let tab = h.state().tabs()[0].id.to_string();
        let menu =
            eframe::egui::AreaState::load(&h.ctx, eframe::egui::Id::new(("sketch_menu", tab)))
                .unwrap()
                .rect();
        assert!(
            (menu.left() - x / ppp).abs() <= 1.0 / 32.0,
            "{ppp} {menu:?}"
        );
        assert!((menu.top() - y / ppp).abs() <= 1.0 / 32.0, "{ppp} {menu:?}");
        // A hundredth of a pixel into its left edge's pixel, and into its top edge's: on it.
        for press in [
            pos2((x + 0.01) / ppp, menu.center().y),
            pos2(menu.center().x, (y + 0.01) / ppp),
        ] {
            left_click_at(&mut h, press);
            step(&mut h);
            assert!(sketch_menu(&h).is_some(), "{ppp} {press:?} {menu:?}");
            assert!(sketch_mode(&h).unwrap().menu_at.is_some(), "{ppp}");
        }
        // A hundredth of a pixel before its left edge's pixel: off it, and it closes.
        let press = pos2((x - 0.01) / ppp, menu.center().y);
        left_click_at(&mut h, press);
        step(&mut h);
        assert!(sketch_menu(&h).is_none(), "{ppp} {press:?} {menu:?}");
        assert_eq!(sketch_mode(&h).unwrap().menu_at, None, "{ppp}");
        assert_eq!(never_waits::waits(), 0);
    }
}

// ---- M5.11's wiring, the prototype's look: the Modify box's icons, the over-define
// prompt's head, rows, and keys, and the Sketch Palette inside the viewport ----

/// The text of every text shape the last frame painted.
fn painted_texts(h: &AppHarness) -> Vec<String> {
    painted(h)
        .iter()
        .filter_map(|shape| match shape {
            eframe::egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_modify_boxs_buttons_are_its_icons_named_accept_and_cancel_with_no_glyph_drawn() {
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // The app's fonts and icons, so text and icons have their sizes.
    silico_app::theme::apply(&h.ctx);
    step(&mut h);
    h.key_press(eframe::egui::Key::D);
    step(&mut h);
    sketch_events(&f, &mut h, &clicked(50.0, 20.0));
    sketch_events(&f, &mut h, &clicked(60.0, 20.0));
    step_until(&mut h, "the Modify box drawn", |h| {
        h.query_by_role_and_label(Role::Dialog, "Modify dimension")
            .is_some()
    });
    for _ in 0..3 {
        step(&mut h);
    }
    let dialog = h.get_by_role_and_label(Role::Dialog, "Modify dimension");
    // Its two buttons by their names, as the prototype's `aria-label`s, each 22 square.
    for name in ["Accept", "Cancel"] {
        let button = dialog.get_by_role_and_label(Role::Button, name).rect();
        assert!(
            (button.width() - 22.0).abs() < 1e-3 && (button.height() - 22.0).abs() < 1e-3,
            "{name} {button:?}"
        );
    }
    // Drawn as icons: no ✓, ✕, or a font's stand-in for one, among the frame's text.
    let texts = painted_texts(&h);
    for glyph in ["✓", "✕", "?"] {
        assert!(!texts.iter().any(|t| t == glyph), "{glyph} in {texts:?}");
    }
    assert!(texts.iter().any(|t| t == "20"), "{texts:?}");
    // Cancel skips: the measured value placed, the box closed.
    dialog.get_by_role_and_label(Role::Button, "Cancel").click();
    step(&mut h);
    settled(&mut h);
    step_until(&mut h, "the box closed", |h| {
        sketch_mode(h).and_then(|mode| mode.modify_view()).is_none()
    });
    read_with_constraints(&mut h, 5);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_over_define_prompts_rows_share_their_edges_its_keys_are_16_high_at_their_tops_and_its_head_has_the_warning()
 {
    use silico_app::sketch::dimension::ModifyAction;
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // The app's fonts and icons, so text and icons have their sizes.
    silico_app::theme::apply(&h.ctx);
    step(&mut h);
    h.key_press(eframe::egui::Key::D);
    step(&mut h);
    sketch_events(&f, &mut h, &clicked(50.0, 20.0));
    sketch_events(&f, &mut h, &clicked(60.0, 20.0));
    step_until(&mut h, "the Modify box open", |h| {
        sketch_mode(h).and_then(|mode| mode.modify_view()).is_some()
    });
    modify(&f, &mut h, ModifyAction::Typed("99".to_owned()));
    modify(&f, &mut h, ModifyAction::Accept);
    settled(&mut h);
    // Laid out: a modal is centred, so it moves as it's first sized.
    let rows = |h: &AppHarness| {
        prompt(h).map(|p| {
            ["Make it driven", "Leave it driving", "Cancel"]
                .map(|title| p.get_by_role_and_label(Role::Button, title).rect())
        })
    };
    let last = std::cell::Cell::new(rows(&h));
    step_until(&mut h, "the prompt laid out", |h| {
        let now = rows(h);
        now.is_some() && now == last.replace(now)
    });
    let rows = rows(&h).unwrap();
    let prompt = prompt(&h).unwrap();
    // The prototype's 440 wide: its rows the width inside its 16-point sides and border,
    // 406, each with the same left and right edges.
    for row in &rows {
        assert!((row.width() - 406.0).abs() < 1e-3, "{rows:?}");
        assert!((row.left() - rows[0].left()).abs() < 1e-3, "{rows:?}");
        assert!((row.right() - rows[0].right()).abs() < 1e-3, "{rows:?}");
    }
    // Each key's badge 16 high (`.kbd`), at its row's top inside its border and padding
    // (1 + 8), and its right inside them (1 + 10).
    for (row, key) in rows.iter().zip(["1", "2", "Esc"]) {
        let badge = prompt.get_by_label(key).rect();
        assert!((badge.height() - 16.0).abs() < 1e-3, "{key} {badge:?}");
        assert!(badge.width() >= 16.0 - 1e-3, "{key} {badge:?}");
        assert!(
            (badge.top() - (row.top() + 9.0)).abs() < 1e-3,
            "{key} {badge:?} {row:?}"
        );
        assert!(
            (badge.right() - (row.right() - 11.0)).abs() < 1e-3,
            "{key} {badge:?} {row:?}"
        );
    }
    // The second row's words wrap beside its badge: the row taller than a line's two.
    assert!(rows[1].height() > rows[2].height() + 8.0, "{rows:?}");
    // The head: the title, after the warning's icon (an image, no text).
    let title = prompt
        .get_by_label("This dimension over-defines Box")
        .rect();
    assert!(
        title.left() >= rows[0].left() + 14.0 + 8.0 - 1e-3,
        "{title:?} {rows:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

/// The app at `size` points with `text` open from `name`, its timeline read.
fn opened_sized(f: &Fixture, name: &str, text: &str, size: (f32, f32)) -> AppHarness {
    let path = f.file(name, text);
    let app = App::new(Arc::clone(&f.sessions));
    let mut h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size(size)
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    };
    h.state_mut().open(path.to_str().unwrap(), false);
    step_until(&mut h, "the timeline read", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.timeline.is_some())
    });
    h
}

#[test]
fn in_a_short_window_the_sketch_palette_stays_inside_the_viewport_and_its_last_section_scrolls_into_it()
 {
    let f = fixture();
    let mut h = opened_sized(&f, "boxed.silico", BOXED, (1280.0, 560.0));
    // The app's fonts, so the palette's text has its sizes.
    silico_app::theme::apply(&h.ctx);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    for _ in 0..3 {
        step(&mut h);
    }
    let viewport = view(&h).frame().unwrap().rect;
    let palette = palette_rect(&h);
    // Inside the viewport, its bottom at most the prototype's 12 above the viewport's.
    assert!(viewport.contains_rect(palette), "{palette:?} {viewport:?}");
    // 212 wide, 12 from the viewport's right, and at least 12 from its bottom: to half a
    // pixel, as egui puts an area's corner on one.
    let half = 0.5 / h.ctx.pixels_per_point();
    assert!((palette.width() - 212.0).abs() < 1e-3, "{palette:?}");
    let right = viewport.right() - 12.0;
    assert!(
        (palette.right() - right).abs() <= half,
        "{palette:?} {viewport:?}"
    );
    // Its sections past it: 12 from the viewport's bottom.
    let bottom = viewport.bottom() - 12.0;
    assert!(
        (palette.bottom() - bottom).abs() <= half,
        "{palette:?} {viewport:?}"
    );
    // Its title stays; the legend's last row, Construction, is below it, out of sight.
    let title = h.get_by_label("SKETCH PALETTE").rect();
    let last = |h: &AppHarness| h.get_by_label("Construction").rect();
    assert!(
        last(&h).bottom() > palette.bottom(),
        "{:?} {palette:?}",
        last(&h)
    );
    // Scrolled over the palette: the legend's last row comes into it, the title where it was.
    h.hover_at(palette.center());
    for _ in 0..10 {
        h.event(eframe::egui::Event::MouseWheel {
            unit: eframe::egui::MouseWheelUnit::Point,
            delta: eframe::egui::vec2(0.0, -200.0),
            phase: eframe::egui::TouchPhase::Move,
            modifiers: eframe::egui::Modifiers::NONE,
        });
        step(&mut h);
    }
    for _ in 0..30 {
        step(&mut h);
    }
    let shown = last(&h);
    assert!(palette.contains_rect(shown), "{shown:?} {palette:?}");
    assert_eq!(palette_rect(&h), palette);
    assert_eq!(h.get_by_label("SKETCH PALETTE").rect(), title);
    // The window made taller: the palette grows to the new cap, not held at its last size.
    h.set_size(eframe::egui::vec2(1280.0, 640.0));
    for _ in 0..5 {
        step(&mut h);
    }
    let viewport = view(&h).frame().unwrap().rect;
    let taller = palette_rect(&h);
    assert!(
        taller.height() > palette.height() + 40.0,
        "{taller:?} {palette:?}"
    );
    let bottom = viewport.bottom() - 12.0;
    assert!(
        (taller.bottom() - bottom).abs() <= half,
        "{taller:?} {viewport:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

/// How many undos and redos the tab has sent.
fn steps_sent(h: &AppHarness) -> usize {
    h.state().tabs()[0]
        .sent
        .iter()
        .filter(|sent| matches!(sent, Sent::Undo(_) | Sent::Redo(_)))
        .count()
}

/// Ctrl+Z, then Ctrl+Y, each its own frame.
fn undo_redo_keys(h: &mut AppHarness) {
    for key in [eframe::egui::Key::Z, eframe::egui::Key::Y] {
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, key);
        step(h);
    }
}

#[test]
fn mid_drag_undo_and_redo_are_off_and_their_keys_send_nothing_and_once_it_ends_both_are_back() {
    use silico_app::sketch::PointerAction::{Move, Press, Release};
    let f = fixture();
    counting(&f);
    let mut h = boxed(&f);
    // Two corners dragged, each one edit, and the second undone: an edit to undo and one
    // to redo, both offered.
    for (from, to) in [([50.0, 30.0], [52.0, 32.0]), ([10.0, 10.0], [12.0, 12.0])] {
        sketch_events(
            &f,
            &mut h,
            &[
                pointer(Press, from[0], from[1]),
                pointer(Move, to[0], to[1]),
            ],
        );
        drag_frames(&mut h, 1);
        sketch_events(&f, &mut h, &[pointer(Release, to[0], to[1])]);
        settled(&mut h);
        step_until(&mut h, "the drag over", |h| the_drag(h).is_none());
    }
    step_until(&mut h, "Undo offered", |h| offered(h, "Undo"));
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    step_until(&mut h, "both offered", |h| {
        offered(h, "Undo") && offered(h, "Redo")
    });
    // The second corner, back at (10, 10), pressed and dragged: neither is offered, by
    // button or key, while the drag is under way.
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 10.0, 10.0), pointer(Move, 14.0, 14.0)],
    );
    drag_frames(&mut h, 1);
    assert!(the_drag(&h).is_some_and(|d| d.in_flight()));
    let sent = steps_sent(&h);
    for _ in 0..5 {
        step(&mut h);
        assert!(!offered(&h, "Undo") && !offered(&h, "Redo"));
        let tab = &h.state().tabs()[0];
        assert!(!tab.undoable() && !tab.redoable());
    }
    undo_redo_keys(&mut h);
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(steps_sent(&h), sent);
    assert!(the_drag(&h).is_some_and(|d| d.in_flight()));
    // Esc ends it from its begin state: nothing changed, and both are back.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    step_until(&mut h, "the drag over", |h| the_drag(h).is_none());
    step_until(&mut h, "both offered again", |h| {
        offered(h, "Undo") && offered(h, "Redo")
    });
    assert_eq!(steps_sent(&h), sent);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn after_the_over_define_prompts_cancel_redo_is_off_and_its_key_sends_nothing_but_an_ordinary_undo_offers_it()
 {
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    let (id, _, top) = fully_defined(&f, &mut h);
    let before = text_of(&f, &h);
    // A third length, over defining it, then the prompt's Cancel (Esc): its `undo`.
    over_define(&f, &mut h, 65.0, "35", 9);
    step_until(&mut h, "the prompt", |h| prompt(h).is_some());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Undo(None)));
    assert_eq!(text_of(&f, &h), before);
    step_until(&mut h, "the prompt closed", |h| prompt(h).is_none());
    // The session holds the dimension to redo, as a status of the undo's revision counts
    // it; Redo stays off all the same, as SolidWorks' Cancel leaves nothing to redo.
    step_until(&mut h, "a status of the undo", |h| {
        let tab = &h.state().tabs()[0];
        tab.transaction().is_some_and(|t| t.redo == 1) && offered(h, "Undo")
    });
    for _ in 0..5 {
        step(&mut h);
        assert!(!offered(&h, "Redo"));
        assert!(!h.state().tabs()[0].redoable());
    }
    let sent = steps_sent(&h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Y);
    step(&mut h);
    settled(&mut h);
    assert_eq!(steps_sent(&h), sent);
    assert_eq!(text_of(&f, &h), before);
    // An ordinary undo (the top's length) offers Redo, which gives it back, and no further:
    // the conflicting dimension never comes back.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    settled(&mut h);
    assert_eq!(steps_sent(&h), sent + 1);
    assert!(
        lengths(&sketch_of_text(&f, &h, &id))
            .iter()
            .all(|l| l.1 != top)
    );
    step_until(&mut h, "Redo offered", |h| offered(h, "Redo"));
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Y);
    step(&mut h);
    settled(&mut h);
    assert_eq!(steps_sent(&h), sent + 2);
    assert_eq!(text_of(&f, &h), before);
    step_until(&mut h, "a status of the redo", |h| {
        h.state().tabs()[0]
            .transaction()
            .is_some_and(|t| t.redo == 1)
    });
    for _ in 0..5 {
        step(&mut h);
        assert!(!offered(&h, "Redo"));
    }
    assert_eq!(never_waits::waits(), 0);
}

/// The boxed sketch padded: its rebuild after a drag asks the kernel for the pad.
const BOXED_PADDED: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Box", "plane": "xy", "points": [{"id": "pa000001", "x": 10, "y": 10, "construction": false}, {"id": "pa000002", "x": 50, "y": 10, "construction": false}, {"id": "pa000003", "x": 50, "y": 30, "construction": false}, {"id": "pa000004", "x": 10, "y": 30, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}]},
    {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}
  ]
}
"#;

#[test]
fn a_released_drags_edit_is_undone_though_the_rebuild_after_it_is_stopped_before_its_read() {
    use silico_app::sketch::PointerAction::{Move, Press, Release};
    let f = fixture();
    counting(&f);
    let (mut h, _) = f.opened("padded.silico", BOXED_PADDED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Box"));
    double_click(&mut h, "Browser", "Box");
    step_until(&mut h, "sketch mode", |h| sketch_mode(h).is_some());
    sketch_read(&mut h);
    let text = text_of(&f, &h);
    sketch_events(
        &f,
        &mut h,
        &[pointer(Press, 50.0, 30.0), pointer(Move, 52.0, 32.0)],
    );
    drag_frames(&mut h, 1);
    // Every kernel request held from here: the drag's end, the sketch worker's, answers;
    // the rebuild after it, which pads the moved sketch, doesn't.
    let hold = f.kernel.hold(|_| true);
    sketch_events(&f, &mut h, &[pointer(Release, 52.0, 32.0)]);
    step_until(&mut h, "the drag's end answered", |h| {
        the_drag(h).is_some_and(|d| d.ended() && !d.in_flight())
    });
    assert_ne!(text_of(&f, &h), text);
    // Stop, before the rebuild publishes: the read of the drag's edit never comes.
    let stop_on = |h: &AppHarness| {
        h.query_all_by_role_and_label(Role::Button, "Stop")
            .last()
            .is_some_and(|stop| !stop.accesskit_node().is_disabled())
    };
    step_until(&mut h, "Stop on", stop_on);
    h.query_all_by_role_and_label(Role::Button, "Stop")
        .last()
        .unwrap()
        .click();
    step_until(&mut h, "the rebuild stopped, a status of the edit", |h| {
        rebuild_stopped(h)
            && h.state().tabs()[0]
                .transaction()
                .is_some_and(|t| t.undo == 1)
    });
    for _ in 0..5 {
        step(&mut h);
    }
    // The drag waits still for that read, but its edit has landed: Undo is offered, and
    // Ctrl+Z sends it.
    assert!(the_drag(&h).is_some_and(|d| !d.under_way()));
    assert!(offered(&h, "Undo"));
    let sent = steps_sent(&h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step(&mut h);
    assert_eq!(steps_sent(&h), sent + 1);
    assert_eq!(h.state().tabs()[0].sent.last(), Some(&Sent::Undo(None)));
    hold.release();
    settled(&mut h);
    assert_eq!(text_of(&f, &h), text);
    assert_eq!(never_waits::waits(), 0);
}
