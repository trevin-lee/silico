//! Hostile input for untitled parts (docs/design/m5.0-app.md §4, New): a Save panel's answer
//! for a tab gone or closing, Command+S twice and a close asked while its panel is open, a
//! Save As to a file another tab holds or one the core refuses, an untitled part's export to
//! paths that aren't full, New past the session limit, and a quit whose panel answers with a
//! path the app can't name. Driven as the app's own tests drive it, through `egui_kittest`
//! on the fake kernel, the native dialog a stand-in, every frame on a thread marked so that
//! a wait on a command fails. Expected values by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/ui.rs"]
mod ui;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use silico_app::App;
use silico_app::browse::{Answer, Ask, Browse, Pick};
use silico_app::export::ExportForm;
use silico_core::cache::Cache;
use silico_core::command::{SessionId, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches, never_waits};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;
use ui::{
    answer_question, anywhere, begin_sketch_on, close_window, new_part, step, step_until, tab_named,
};

const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

type Fake = Arc<FakeKernel>;
type AppHarness = Harness<'static, App<Fake>>;

struct Fixture {
    dir: TempDir,
    sessions: Arc<Sessions<Fake>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let solver = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    Fixture {
        dir,
        sessions: Arc::new(sessions),
    }
}

/// The stand-in's state: what each dialog was asked, the answer the test gave (`None`
/// until it does), and the waker of the last poll.
#[derive(Default)]
struct Dialogs {
    asked: Vec<Ask>,
    answer: Option<Option<PathBuf>>,
    waker: Option<std::task::Waker>,
}

/// A native dialog that answers when the test says, from a thread of its own.
#[derive(Clone, Default)]
struct FakeBrowser(Arc<Mutex<Dialogs>>);

impl Browse for FakeBrowser {
    fn browse(&mut self, ask: &Ask) -> Option<Answer> {
        let mut dialogs = self.0.lock().unwrap();
        dialogs.asked.push(ask.clone());
        dialogs.answer = None;
        Some(Box::pin(FakeAnswer(Arc::clone(&self.0))))
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
}

impl Fixture {
    fn app(&self, browser: &FakeBrowser) -> AppHarness {
        let app = App::new(Arc::clone(&self.sessions)).browsing(Box::new(browser.clone()));
        let _marked = never_waits::marked();
        let h = Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app);
        // The app's look, as main.rs gives it: its fonts above all.
        silico_app::theme::apply(&h.ctx);
        h
    }
}

fn steps(h: &mut AppHarness, n: usize) {
    for _ in 0..n {
        step(h);
    }
}

fn press_keys(h: &mut AppHarness, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
}

/// A sketch added to the part `name`, shown, as a part is begun sketch-first (the app makes
/// no primitive: Trevin's decision of 2026-11-09): CREATE's Create Sketch on the XY plane,
/// then Finish Sketch (Command+Enter), which keeps it, empty, as "Sketch1". The part `name`
/// is then unsaved.
fn add_sketch(h: &mut AppHarness, name: &str) {
    // Create Sketch on once the part's timeline is read.
    begin_sketch_on(h, name, "XY plane");
    press_keys(h, Modifiers::COMMAND, Key::Enter);
    step_until(h, "unsaved", |h| {
        let tab = tab_named(h, name);
        tab.sketch.is_none() && tab.edits.idle() && tab.unsaved()
    });
}

/// Frames stepped until the native dialog has been asked `n` times in all.
#[track_caller]
fn asked_times(h: &mut AppHarness, browser: &FakeBrowser, n: usize) {
    step_until(h, "the panel asked", |_| browser.asked().len() >= n);
    assert_eq!(browser.asked().len(), n);
    assert_eq!(browser.asked()[n - 1].pick, Pick::Save);
}

fn closing_told(h: &AppHarness) -> bool {
    h.output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .is_some_and(|viewport| {
            viewport
                .commands
                .contains(&eframe::egui::ViewportCommand::Close)
        })
}

#[test]
fn a_save_panels_pick_for_a_part_closed_meanwhile_writes_nothing_and_says_nothing() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app(&browser);
    step(&mut h);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    press_keys(&mut h, Modifiers::COMMAND, Key::S);
    asked_times(&mut h, &browser, 1);
    // The portal's panel leaves the window to be used: the part closed, its edits dropped.
    press_keys(&mut h, Modifiers::COMMAND, Key::W);
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to Untitled before closing?")
    });
    answer_question(&mut h, "Don't save");
    step_until(&mut h, "it closed", |h| h.state().tabs().is_empty());
    let late = f.dir.path().join("late.silico");
    browser.answer(Some(&late));
    steps(&mut h, 20);
    assert!(!late.exists());
    assert!(h.state().tabs().is_empty());
    assert_eq!(h.state().message(), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_panels_pick_for_a_part_whose_dont_save_is_still_running_writes_nothing() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app(&browser);
    step(&mut h);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    press_keys(&mut h, Modifiers::COMMAND, Key::S);
    asked_times(&mut h, &browser, 1);
    press_keys(&mut h, Modifiers::COMMAND, Key::W);
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to Untitled before closing?")
    });
    // Don't save, and the pick answering in the very next frame, the rollback and close
    // still under way.
    answer_question(&mut h, "Don't save");
    let late = f.dir.path().join("late.silico");
    browser.answer(Some(&late));
    step_until(&mut h, "it closed", |h| h.state().tabs().is_empty());
    steps(&mut h, 10);
    assert!(!late.exists());
    assert_eq!(h.state().message(), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_close_asked_while_the_parts_save_panel_is_open_is_taken_on_by_that_panel() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app(&browser);
    step(&mut h);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    // Command+S, then Close's Save while that panel is open: no second panel, and the one
    // open closes the part once it's saved.
    press_keys(&mut h, Modifiers::COMMAND, Key::S);
    asked_times(&mut h, &browser, 1);
    press_keys(&mut h, Modifiers::COMMAND, Key::W);
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to Untitled before closing?")
    });
    answer_question(&mut h, "Save");
    steps(&mut h, 10);
    assert_eq!(browser.asked().len(), 1);
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "saved and closed", |h| h.state().tabs().is_empty());
    assert!(part.is_file());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancel_of_a_save_panel_a_close_took_on_keeps_the_part() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app(&browser);
    step(&mut h);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    press_keys(&mut h, Modifiers::COMMAND, Key::S);
    asked_times(&mut h, &browser, 1);
    press_keys(&mut h, Modifiers::COMMAND, Key::W);
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to Untitled before closing?")
    });
    answer_question(&mut h, "Save");
    browser.answer(None);
    steps(&mut h, 20);
    let tab = tab_named(&h, "Untitled");
    assert!(tab.unsaved() && !tab.closing());
    assert_eq!(tab.saves, 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_as_to_a_file_another_tab_holds_is_refused_said_and_leaves_the_part_untitled() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let held = f.dir.path().join("held.silico");
    std::fs::write(&held, EXAMPLE).unwrap();
    let mut h = f.app(&browser);
    h.state_mut().open(held.to_str().unwrap(), false);
    step_until(&mut h, "its tab", |h| h.state().tabs().len() == 1);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    press_keys(&mut h, Modifiers::COMMAND, Key::S);
    asked_times(&mut h, &browser, 1);
    browser.answer(Some(&held));
    step_until(&mut h, "refused", |h| {
        tab_named(h, "Untitled").message.is_some()
    });
    let said = tab_named(&h, "Untitled").message.clone().unwrap();
    assert!(
        said.contains("held.silico\" is open in session s1"),
        "{said}"
    );
    let tab = tab_named(&h, "Untitled");
    assert_eq!(tab.path, None);
    assert!(tab.unsaved());
    assert_eq!(std::fs::read_to_string(&held).unwrap(), EXAMPLE);
    // Not stuck: saved elsewhere next.
    press_keys(&mut h, Modifiers::COMMAND, Key::S);
    asked_times(&mut h, &browser, 2);
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "saved", |h| {
        h.state()
            .tabs()
            .iter()
            .any(|tab| tab.path.as_deref() == part.to_str() && !tab.unsaved())
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_quits_save_to_a_file_another_session_holds_stops_the_quit() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let held = f.dir.path().join("held.silico");
    std::fs::write(&held, EXAMPLE).unwrap();
    // Held by another client of the same sessions (an agent's, say): a tab of the app's
    // holding it would be closed by the quit, letting it go before the save.
    let other = f.sessions.open(&held, false).wait();
    assert!(other.result.is_ok());
    let mut h = f.app(&browser);
    step(&mut h);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    asked_times(&mut h, &browser, 1);
    browser.answer(Some(&held));
    step_until(&mut h, "the quit stopped", |h| {
        h.state().message() == Some("Quit stopped: Untitled wasn't closed")
    });
    steps(&mut h, 20);
    assert!(!closing_told(&h));
    assert!(tab_named(&h, "Untitled").unsaved());
    assert_eq!(std::fs::read_to_string(&held).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_panels_pick_the_core_refuses_is_said_and_the_part_keeps_its_edits() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app(&browser);
    step(&mut h);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    // One in a folder that isn't there, a folder, and one in a rebuild cache.
    let folder = f.dir.path().join("folder.silico");
    std::fs::create_dir(&folder).unwrap();
    let cache = f.dir.path().join(".silico-cache");
    std::fs::create_dir(&cache).unwrap();
    for (at, picked) in [
        f.dir.path().join("absent").join("part.silico"),
        folder.clone(),
        cache.join("part.silico"),
    ]
    .iter()
    .enumerate()
    {
        press_keys(&mut h, Modifiers::COMMAND, Key::S);
        asked_times(&mut h, &browser, at + 1);
        browser.answer(Some(picked));
        step_until(&mut h, "refused", |h| {
            tab_named(h, "Untitled").message.is_some()
        });
        let tab = tab_named(&h, "Untitled");
        assert_eq!(tab.path, None, "{}", picked.display());
        assert!(tab.unsaved(), "{}", picked.display());
        assert!(!picked.is_file(), "{}", picked.display());
    }
    // Not a .silico (a portal's panel needn't add it): not refused, but saved with `.silico`
    // added, and nothing written by the name picked.
    let picked = f.dir.path().join("part.txt");
    let written = f.dir.path().join("part.txt.silico");
    press_keys(&mut h, Modifiers::COMMAND, Key::S);
    asked_times(&mut h, &browser, 4);
    browser.answer(Some(&picked));
    step_until(&mut h, "saved", |h| {
        h.state().tabs().iter().any(|tab| {
            tab.path.as_deref() == written.to_str() && !tab.unsaved() && tab.edits.idle()
        })
    });
    assert!(written.is_file());
    assert!(!picked.exists());
    assert_eq!(never_waits::waits(), 0);
}

#[cfg(unix)]
#[test]
fn a_quits_save_panel_answering_a_path_that_isnt_utf8_stops_the_quit() {
    use std::os::unix::ffi::OsStrExt;
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app(&browser);
    step(&mut h);
    new_part(&mut h);
    add_sketch(&mut h, "Untitled");
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    asked_times(&mut h, &browser, 1);
    let mut bytes = f.dir.path().as_os_str().as_bytes().to_vec();
    bytes.extend_from_slice(b"/\xff.silico");
    browser.answer(Some(&PathBuf::from(std::ffi::OsStr::from_bytes(&bytes))));
    step_until(&mut h, "the quit stopped", |h| {
        h.state().message() == Some("Quit stopped: Untitled wasn't closed")
    });
    steps(&mut h, 20);
    assert!(!closing_told(&h));
    assert!(tab_named(&h, "Untitled").unsaved());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_untitled_parts_export_takes_only_a_full_path() {
    let id = SessionId::parse("s1").unwrap();
    let refused = Some("Give a full path: an untitled part has no folder");
    let mut form = ExportForm::untitled(id, "Untitled", None);
    for typed in [
        ".",
        "..",
        "~/out.3mf",
        "./out.3mf",
        "../out.3mf",
        "C:\\out.3mf",
        "out",
        "\u{0}",
        " /out.3mf",
    ] {
        form.path = typed.to_owned();
        assert_eq!(form.refusal(), refused, "{typed:?}");
        assert_eq!(form.request(&[]), None, "{typed:?}");
    }
    for typed in ["/", "/out.3mf", "//out.3mf", "/a/../out.3mf"] {
        form.path = typed.to_owned();
        assert_eq!(form.refusal(), None, "{typed:?}");
        assert_eq!(
            form.request(&[]).map(|request| request.path),
            Some(PathBuf::from(typed)),
            "{typed:?}"
        );
    }
    // A name with a space: in the Documents folder, as typed.
    let form = ExportForm::untitled(id, "Untitled 2", Some(Path::new("/docs")));
    assert_eq!(form.path, "/docs/Untitled 2.3mf");
}

#[test]
fn new_past_the_session_limit_is_refused_and_said_and_no_name_comes_again() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app(&browser);
    step(&mut h);
    for _ in 0..silico_core::command::MAX_SESSIONS {
        new_part(&mut h);
    }
    // One more: refused, said, no tab.
    press_keys(&mut h, Modifiers::COMMAND, Key::N);
    step_until(&mut h, "refused", |h| h.state().message().is_some());
    steps(&mut h, 5);
    assert_eq!(h.state().tabs().len(), silico_core::command::MAX_SESSIONS);
    let said = h.state().message().unwrap().to_owned();
    assert!(said.contains("sessions are open"), "{said}");
    // One closed, then New: a name past every one given, the refused one's included.
    let last = format!("Untitled {}", silico_core::command::MAX_SESSIONS);
    h.get_by_label(&format!("Close {last}")).click();
    step_until(&mut h, "closed", |h| {
        h.state().tabs().len() == silico_core::command::MAX_SESSIONS - 1
    });
    new_part(&mut h);
    let want = format!("Untitled {}", silico_core::command::MAX_SESSIONS + 2);
    assert!(
        h.state().tabs().iter().any(|tab| tab.name() == want),
        "{:?}",
        h.state()
            .tabs()
            .iter()
            .map(|tab| tab.name().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn save_as_picking_the_documents_own_file_saves_it() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let own = f.dir.path().join("own.silico");
    std::fs::write(&own, EXAMPLE).unwrap();
    let mut h = f.app(&browser);
    h.state_mut().open(own.to_str().unwrap(), false);
    step_until(&mut h, "its timeline", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.timeline.is_some())
    });
    add_sketch(&mut h, "own.silico");
    press_keys(&mut h, Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
    asked_times(&mut h, &browser, 1);
    browser.answer(Some(&own));
    step_until(&mut h, "saved", |h| !tab_named(h, "own.silico").unsaved());
    assert_eq!(tab_named(&h, "own.silico").message, None);
    let text = std::fs::read_to_string(&own).unwrap();
    assert!(text.contains(r#""name": "Sketch1""#), "{text}");
    assert_eq!(never_waits::waits(), 0);
}

#[cfg(unix)]
#[test]
fn save_as_picking_a_link_to_the_documents_own_file_saves_it_and_keeps_its_path() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let own = f.dir.path().join("own.silico");
    std::fs::write(&own, EXAMPLE).unwrap();
    // A link to it, as a panel may answer (or its canonical form, /private/var for /var).
    let link = f.dir.path().join("link.silico");
    std::os::unix::fs::symlink(&own, &link).unwrap();
    let mut h = f.app(&browser);
    h.state_mut().open(own.to_str().unwrap(), false);
    step_until(&mut h, "its timeline", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.timeline.is_some())
    });
    add_sketch(&mut h, "own.silico");
    press_keys(&mut h, Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
    asked_times(&mut h, &browser, 1);
    browser.answer(Some(&link));
    step_until(&mut h, "saved", |h| !tab_named(h, "own.silico").unsaved());
    steps(&mut h, 5);
    let tab = tab_named(&h, "own.silico");
    assert_eq!(tab.message, None);
    assert_eq!(tab.path.as_deref(), own.to_str());
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    let text = std::fs::read_to_string(&own).unwrap();
    assert!(text.contains(r#""name": "Sketch1""#), "{text}");
    // No transaction left open, nothing more to save.
    assert!(!tab.transaction);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_as_onto_its_own_file_with_no_edits_writes_nothing_and_leaves_it_clean() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let own = f.dir.path().join("own.silico");
    std::fs::write(&own, EXAMPLE).unwrap();
    let mut h = f.app(&browser);
    h.state_mut().open(own.to_str().unwrap(), false);
    step_until(&mut h, "its timeline", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.timeline.is_some())
    });
    press_keys(&mut h, Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
    asked_times(&mut h, &browser, 1);
    browser.answer(Some(&own));
    // Its own Save As began a transaction, refused, rolled back: nothing to save.
    step_until(&mut h, "settled", |h| {
        let tab = tab_named(h, "own.silico");
        !tab.saving_as() && !tab.transaction && !tab.unsettled()
    });
    steps(&mut h, 5);
    let tab = tab_named(&h, "own.silico");
    assert_eq!(tab.message, None);
    assert!(!tab.unsaved());
    assert_eq!(std::fs::read_to_string(&own).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}
