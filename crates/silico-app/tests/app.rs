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
use silico_core::command::wire::Named;
use silico_core::command::{CheckAnswer, CheckModel, ModelChoice, Revision, Running, Sessions};
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
        let h = Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app);
        // The app's look, as main.rs gives it: its fonts above all, without which every
        // text lays out empty and a field's cursor never leaves its start.
        silico_app::theme::apply(&h.ctx);
        h
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
/// cursor is at its end, so what's typed comes after the name.
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

/// The harness's app is in the app's own fonts, as main.rs gives them: without them every
/// text lays out empty, and a field's cursor never leaves its start.
#[test]
fn the_harness_draws_in_the_apps_fonts() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    let (inter, proportional) = h.ctx.fonts(|fonts| {
        let definitions = fonts.definitions();
        (
            definitions.font_data.contains_key("Inter"),
            definitions
                .families
                .get(&eframe::egui::FontFamily::Proportional)
                .cloned(),
        )
    });
    assert!(inter, "Inter isn't loaded");
    assert_eq!(proportional, Some(vec!["Inter".to_owned()]));
    // And typed text lands after a field's text, as for a person.
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "12");
    assert_eq!(name_field(&h).value().as_deref(), Some("Pin12"));
}

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

/// The prototype's `#main` (styles.css): `grid-template-columns: var(--browser-w) 1fr
/// var(--right-w)`, `--browser-w: 248px`, `--right-w: 296px`, every box `border-box`, with
/// `#browser`'s `border-right: 1px` and `#right`'s `border-left: 1px` inside them; `#main`
/// fills the window's width between the toolbar and the timeline, with no padding.
#[test]
fn the_panes_are_the_prototypes_columns_the_browser_248_the_properties_296_and_the_canvas_the_rest()
{
    // The harness draws the app inside a margin of 8 (egui_kittest's `run_ui`:
    // `Frame::central_panel(..).outer_margin(8.0)`), so a harness 16 more each way gives the
    // app the window it's named for, its top left at (8, 8).
    const MARGIN: f32 = 8.0;
    // (window width and height, the canvas's x and width, the right panel's x), from the
    // window's left, by hand: the browser 0 to 248 (its border 247 to 248), the right
    // panel's border the 1 before its panel, the canvas between. At 1280: 1280 − 248 − 296 =
    // 736, the border at 248 + 736 = 984, the panel from 985 (1280 − 295). At 1600: 1600 −
    // 544 = 1056, the panel from 1305. At 500, under the side columns' 544: `1fr` is nothing
    // (`#canvas-wrap`'s `min-width: 0`), the panel's border at 248, the panel from 249 to
    // 544, past the window's right edge. At the window's least size, 884 by 336 (main.rs):
    // 884 − 544 = 340, the panel from 589 (884 − 295).
    for (window, window_h, canvas_w, right_x) in [
        (1280.0, 800.0, 736.0, 985.0),
        (1600.0, 800.0, 1056.0, 1305.0),
        (500.0, 800.0, 0.0, 249.0),
        (884.0, 336.0, 340.0, 589.0),
    ] {
        // Sessions of its own for each window: one holds the file it opened.
        let f = fixture();
        let path = f.file("example.silico", EXAMPLE);
        let app = App::new(Arc::clone(&f.sessions));
        let mut h = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size((window + 2.0 * MARGIN, window_h + 2.0 * MARGIN))
                .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
        };
        h.state_mut().open(path.to_str().unwrap(), false);
        step_until(&mut h, "the panes", |h| {
            h.query_by_role_and_label(Role::Pane, "Viewport").is_some()
        });
        step(&mut h);
        let pane = |label| {
            h.get_by_role_and_label(Role::Pane, label)
                .rect()
                .translate(-eframe::egui::vec2(MARGIN, MARGIN))
        };
        let (browser, canvas, right) = (pane("Browser"), pane("Viewport"), pane("Properties"));
        let at = format!("at {window} by {window_h}");
        assert_eq!((browser.left(), browser.width()), (0.0, 247.0), "{at}");
        assert_eq!((canvas.left(), canvas.width()), (248.0, canvas_w), "{at}");
        assert_eq!((right.left(), right.width()), (right_x, 295.0), "{at}");
        // Their top the toolbar's bottom: `--top-bar-h` 30, then the toolbar's
        // `--toolbar-tabs-h` 24, `--toolbar-h` 52, and its `border-bottom` 1.
        assert_eq!(browser.top(), 30.0 + 24.0 + 52.0 + 1.0, "{at}");
        // Side by side, the three the same height, down to the timeline, which runs the
        // window's width: to 1 above its 40, egui's separator line, the timeline's border,
        // drawn outside its panel (where `#timeline`'s `border-top` is inside its 40); the
        // status bar, `--status-h` 24, under it.
        let timeline = pane("Timeline");
        for rect in [canvas, right] {
            assert_eq!(rect.y_range(), browser.y_range(), "{at}");
        }
        assert_eq!(browser.bottom() + 1.0, timeline.top(), "{at}");
        assert_eq!((timeline.left(), timeline.right()), (0.0, window), "{at}");
        assert_eq!(timeline.bottom(), window_h - 24.0, "{at}");
        // The viewport draws in its pane's rect, once it has drawn a frame.
        if canvas_w > 0.0 {
            step_until(&mut h, "the viewport's frame", |h| {
                view(h).frame().is_some()
            });
            assert_eq!(
                the_viewport(&h),
                h.get_by_role_and_label(Role::Pane, "Viewport").rect(),
                "{at}"
            );
        }
        assert_eq!(never_waits::waits(), 0);
    }
}

/// The prototype's columns are fixed (`#main`'s `grid-template-columns`, no splitter): either
/// border, the browser's (247 to 248) or the properties' (984 to 985 at 1280), hovered shows
/// no resize cursor (`#browser` and `#right` set none), and a drag along it moves neither,
/// by hand the same 247, 736, and 295 wide after as before.
#[test]
fn a_drag_on_a_panes_border_resizes_no_column() {
    use eframe::egui::{Event, Modifiers, PointerButton, pos2, vec2};
    const MARGIN: f32 = 8.0;
    let f = fixture();
    let path = f.file("example.silico", EXAMPLE);
    let app = App::new(Arc::clone(&f.sessions));
    let mut h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0 + 2.0 * MARGIN, 800.0 + 2.0 * MARGIN))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    };
    h.state_mut().open(path.to_str().unwrap(), false);
    step_until(&mut h, "the panes", |h| {
        h.query_by_role_and_label(Role::Pane, "Viewport").is_some()
    });
    step(&mut h);
    let widths = |h: &Harness<'_, App<Fake>>| {
        ["Browser", "Viewport", "Properties"]
            .map(|label| h.get_by_role_and_label(Role::Pane, label).rect().width())
    };
    assert_eq!(widths(&h), [247.0, 736.0, 295.0]);
    let middle = MARGIN + 30.0 + 24.0 + 52.0 + 1.0 + 200.0;
    for border in [247.5, 984.5] {
        let from = pos2(MARGIN + border, middle);
        h.hover_at(from);
        step(&mut h);
        assert_ne!(
            h.output().platform_output.cursor_icon,
            eframe::egui::CursorIcon::ResizeHorizontal,
            "the border at {border}"
        );
        h.event(Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
        for i in 1..=8u8 {
            h.hover_at(from + vec2(10.0 * f32::from(i), 0.0));
            step(&mut h);
        }
        h.event(Event::PointerButton {
            pos: from + vec2(80.0, 0.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
        step(&mut h);
        assert_eq!(widths(&h), [247.0, 736.0, 295.0], "the border at {border}");
    }
    assert_eq!(never_waits::waits(), 0);
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
    assert!(create_off(&mut h, "Box"));
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
            && shows(h, "Timeline", "HoleX")
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
                        .is_some_and(|features| features.iter().any(|f| f.name == "HoleX"))
                })
    });
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains(r#""name": "HoleX""#), "{saved}");
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
        anywhere(h, "example.silico *") && shows(h, "Timeline", "HoleY")
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
        anywhere(h, "example.silico *") && shows(h, "Timeline", "Pins")
    });
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#);
    std::fs::write(&path, &theirs).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    assert!(shows(&h, "Timeline", "Pins"));
    assert!(shows(&h, "Timeline", "Bore"));
    // Reload: the edits dropped, the agent's file shown, never written over.
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
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

#[test]
fn a_reload_with_nothing_held_is_announced_in_purple_until_dismissed_or_overtaken() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let reloaded = RELOADED;
    assert!(!anywhere(&h, reloaded));
    // No edits held: the session reloads it itself, and the banner says so, from the
    // revision it opened at (1) to the one its reload gave (2).
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Bore""#),
    )
    .unwrap();
    step_until(&mut h, "the reload announced", |h| {
        shows(h, "Timeline", "Bore") && anywhere(h, reloaded)
    });
    assert!(anywhere(
        &h,
        "Revision 1 → 2. You had no unsaved edits, so nothing of yours was lost. Changed \
         features are marked ● until you look at them."
    ));
    // It stays until it's dismissed.
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(anywhere(&h, reloaded));
    h.get_by_role_and_label(Role::Button, "Dismiss").click();
    step_until(&mut h, "dismissed", |h| !anywhere(h, reloaded));
    // Reloaded again, then changed on disk with an edit held: the amber banner takes its
    // place, and once the edits are kept the purple one doesn't come back.
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#),
    )
    .unwrap();
    step_until(&mut h, "the second reload announced", |h| {
        shows(h, "Timeline", "Drill") && anywhere(h, reloaded)
    });
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "Pins")
    });
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Theirs""#),
    )
    .unwrap();
    let changed = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, changed));
    assert!(!anywhere(&h, reloaded));
    h.get_by_role_and_label(Role::Button, "Keep my edits…")
        .click();
    step(&mut h);
    for _ in 0..10 {
        step(&mut h);
        assert!(!anywhere(&h, changed), "the amber banner came back");
        assert!(!anywhere(&h, reloaded), "the purple banner came back");
    }
    assert_eq!(never_waits::waits(), 0);
}

/// The first cell of the row (a banner's, or a modal's list) one of whose later cells
/// reads `text`: the label at its top, leftmost, left of it, outside the side panes.
fn row_feature(h: &AppHarness, text: &str) -> String {
    let cell = h.get_by_label(text).rect();
    h.root()
        .query_all(egui_kittest::kittest::by().recursive(true))
        .filter(|node| {
            ["Timeline", "Browser", "Properties"]
                .iter()
                .all(|pane| !in_pane(node, pane))
        })
        .filter_map(|node| {
            let node = node.accesskit_node();
            let label = node.label().or_else(|| node.value())?;
            let rect = node.bounding_box()?;
            ((rect.y0 - f64::from(cell.top())).abs() < 0.5
                && rect.x0 < f64::from(cell.left()) - 0.5)
                .then_some((rect.x0, label))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, label)| label)
        .unwrap_or_else(|| panic!("no row reads {text}"))
}

/// The purple banner's title for the example's reload.
const RELOADED: &str = "Reloaded example.silico: another program changed it on disk";

/// Frames stepped, the purple banner absent after each.
#[track_caller]
fn never_reloaded(h: &mut AppHarness, after: &str) {
    for _ in 0..10 {
        step(h);
        assert!(!anywhere(h, RELOADED), "the purple banner after {after}");
    }
}

/// The example written to `path` as another program would, its Hole named `name`, and
/// frames stepped until the session's reload shows it.
fn written_outside(h: &mut AppHarness, path: &Path, name: &str) {
    std::fs::write(
        path,
        EXAMPLE.replace(r#""name": "Hole""#, &format!(r#""name": "{name}""#)),
    )
    .unwrap();
    step_until(h, "the reload shown", |h| shows(h, "Timeline", name));
}

#[test]
fn the_purple_banner_comes_only_from_the_sessions_own_reload() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    written_outside(&mut h, &path, "Bore");
    step_until(&mut h, "the reload announced", |h| anywhere(h, RELOADED));
    h.get_by_role_and_label(Role::Button, "Dismiss").click();
    step_until(&mut h, "dismissed", |h| !anywhere(h, RELOADED));
    // Each of the person's own changes moves the revision: none is a reload.
    select(&mut h, "Pin");
    type_name(&mut h, "A");
    step_until(&mut h, "the edit", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "PinA")
    });
    never_reloaded(&mut h, "an edit");
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step_until(&mut h, "undone", |h| shows(h, "Timeline", "Pin"));
    never_reloaded(&mut h, "an undo");
    type_name(&mut h, "B");
    step_until(&mut h, "the second edit", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "PinB")
    });
    h.get_by_role_and_label(Role::Button, "Save").click();
    step_until(&mut h, "saved", |h| !anywhere(h, "example.silico *"));
    never_reloaded(&mut h, "a save");
    assert!(std::fs::read_to_string(&path).unwrap().contains("PinB"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_reload_the_person_asked_for_shows_no_purple_banner() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let changed = "example.silico changed on disk while you have unsaved edits.";
    // Reload, discard my edits: the reload is the person's.
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| h.state().tabs()[0].transaction);
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#),
    )
    .unwrap();
    step_until(&mut h, "the change announced", |h| anywhere(h, changed));
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
    step_until(&mut h, "reloaded", |h| {
        shows(h, "Timeline", "Drill") && !anywhere(h, "example.silico *")
    });
    never_reloaded(&mut h, "Reload");
    // Keep my edits…, then Revert: the reload is the person's too.
    type_name(&mut h, "t");
    step_until(&mut h, "the edit held again", |h| {
        h.state().tabs()[0].transaction && shows(h, "Timeline", "Pint")
    });
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Bit""#),
    )
    .unwrap();
    step_until(&mut h, "the second change announced", |h| {
        anywhere(h, changed)
    });
    h.get_by_role_and_label(Role::Button, "Keep my edits…")
        .click();
    step_until(&mut h, "kept", |h| !anywhere(h, changed));
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step_until(&mut h, "reverted", |h| {
        shows(h, "Timeline", "Bit") && !anywhere(h, "example.silico *")
    });
    never_reloaded(&mut h, "Revert");
    // Another program's change after them is announced again.
    written_outside(&mut h, &path, "Bore");
    step_until(&mut h, "the reload announced", |h| anywhere(h, RELOADED));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_dismisses_the_purple_banner_before_it_clears_the_selection() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    written_outside(&mut h, &path, "Bore");
    step_until(&mut h, "the reload announced", |h| anywhere(h, RELOADED));
    // A feature selected: the first Esc dismisses the banner, the selection staying (the
    // prototype's `escChain`: the reloaded banner before the selection).
    select(&mut h, "Pin");
    step_until(&mut h, "settled", |h| {
        let tab = &h.state().tabs()[0];
        tab.edits.idle() && !tab.stoppable()
    });
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "dismissed", |h| !anywhere(h, RELOADED));
    for _ in 0..5 {
        step(&mut h);
    }
    let selected = h.state().tabs()[0].selection().feature().cloned();
    assert!(selected.is_some(), "the first Esc cleared the selection");
    assert!(!anywhere(&h, RELOADED));
    // The next clears the selection.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(h.state().tabs()[0].selection().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

/// The example with its Pin's radius 5, as an agent's edits start from it.
fn pinned() -> String {
    EXAMPLE.replace(r#""radius": 2.5"#, r#""radius": 5"#)
}

/// [`pinned`] as an agent leaves it: the Pin's radius 3, and the Hole's note for an M6 bolt.
fn agents_pin() -> String {
    pinned()
        .replace(r#""radius": 5"#, r#""radius": 3"#)
        .replace("an M5 bolt", "an M6 bolt")
}

/// The note's change, as the banner lists it and the marks say it: each value as the file
/// writes it, quoted.
const NOTE_CHANGE: &str = r#""Clearance for an M5 bolt" → "Clearance for an M6 bolt""#;

#[test]
fn a_reload_lists_what_changed_by_feature_and_field_each_value_as_the_file_writes_it() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", &pinned());
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    std::fs::write(&path, agents_pin()).unwrap();
    step_until(&mut h, "the changes listed", |h| seen(h, "5 → 3"));
    assert!(anywhere(&h, RELOADED));
    // Each row: the feature's name, the field, and its values as the file writes them.
    for label in ["Pin", "radius", "5 → 3", "Hole", "note", NOTE_CHANGE] {
        assert!(seen(&h, label), "{label}");
    }
    assert_eq!(h.query_all_by_label("5 → 3").count(), 1);
    assert_eq!(h.query_all_by_label(NOTE_CHANGE).count(), 1);
    // Named by the revision it puts back, never "my version".
    assert!(
        h.query_by_role_and_label(Role::Button, "Restore revision 1")
            .is_some()
    );
    assert!(h.query_by_label_contains("my version").is_none());
    assert_eq!(never_waits::waits(), 0);
}

/// Whether any node is labelled `label`, however many are.
fn seen(h: &AppHarness, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

/// The reload's second line, from revision `from` to `to`, its dot written as the
/// prototype's `.state-dot` is drawn here.
fn reloaded_sub(from: u64, to: u64) -> String {
    format!(
        "Revision {from} → {to}. You had no unsaved edits, so nothing of yours was lost. \
         Changed features are marked ● until you look at them."
    )
}

/// Two blocks, each with the fingerprint it rebuilds to on the fake kernel: Block, 40 × 20
/// × 10 (volume 8000, area 2(800 + 400 + 200) = 2800, centre its middle, (20, 10, 5)), and
/// Plate, 10 × 10 × 2 from x = 50 (volume 200, area 2(100 + 20 + 20) = 280, centre (55, 5,
/// 1)), so neither is marked with a warning.
const BLOCK_AND_PLATE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {"volume": 8000, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}},
    {"id": "p2t8wn4c", "type": "box", "v": 1, "name": "Plate", "position": [50, 0, 0], "length": 10, "width": 10, "height": 2, "fingerprint": {"volume": 200, "area": 280, "center": [55, 5, 1], "bbox": [50, 0, 0, 60, 10, 2], "faces": 6, "edges": 12, "vertices": 8}}
  ]
}
"#;

/// [`BLOCK_AND_PLATE`] as an agent leaves it: Block named Base, and Plate named Lid.
fn blocks_renamed() -> String {
    BLOCK_AND_PLATE
        .replace(r#""name": "Block""#, r#""name": "Base""#)
        .replace(r#""name": "Plate""#, r#""name": "Lid""#)
}

/// The marks of the two renames, as the accessibility tree reads them.
const BASE_MARK: &str = r#"changed on disk: name "Block" → "Base""#;
const LID_MARK: &str = r#"changed on disk: name "Plate" → "Lid""#;

/// How many of `pane`'s features are marked changed on disk.
fn outside_marks(h: &AppHarness, pane: &str) -> usize {
    h.get_by_role_and_label(Role::Pane, pane)
        .query_all_by_label_contains("changed on disk:")
        .count()
}

#[test]
fn the_changed_features_are_marked_in_purple_until_marked_seen_and_esc_dismisses_the_banner() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("blocks.silico", BLOCK_AND_PLATE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Plate"));
    assert_eq!(outside_marks(&h, "Timeline"), 0);
    std::fs::write(&path, blocks_renamed()).unwrap();
    step_until(&mut h, "the marks", |h| {
        shows(h, "Timeline", BASE_MARK) && shows(h, "Timeline", LID_MARK)
    });
    assert!(anywhere(&h, &reloaded_sub(1, 2)));
    // Both bodies, so the browser marks both too, and nothing else.
    step_until(&mut h, "the browser's marks", |h| {
        shows(h, "Browser", BASE_MARK) && shows(h, "Browser", LID_MARK)
    });
    assert_eq!(outside_marks(&h, "Timeline"), 2);
    assert_eq!(outside_marks(&h, "Browser"), 2);
    assert!(seen(&h, r#""Block" → "Base""#) && seen(&h, r#""Plate" → "Lid""#));
    // Selected, and hovered: its properties say what changed, and the mark stays (the
    // prototype's `selectFeature`: seen on select, cleared by the button).
    select(&mut h, "Base");
    assert!(shows(&h, "Properties", "Changed on disk"));
    assert!(shows(&h, "Properties", r#"name "Block" → "Base""#));
    h.get_by_role_and_label(Role::Pane, "Timeline")
        .get_by_label("Base")
        .hover();
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(
        shows(&h, "Timeline", BASE_MARK),
        "the mark went on a look alone"
    );
    // Mark as seen: its marks gone, the Lid's kept.
    h.get_by_role_and_label(Role::Button, "Mark as seen")
        .click();
    step_until(&mut h, "Base's marks gone", |h| {
        !shows(h, "Timeline", BASE_MARK) && !shows(h, "Browser", BASE_MARK)
    });
    assert!(!shows(&h, "Properties", "Changed on disk"));
    assert_eq!(outside_marks(&h, "Timeline"), 1);
    assert!(shows(&h, "Browser", LID_MARK));
    // Esc: the banner first, then the selection (the prototype's `escChain`); nothing
    // modal, and the marks stay. Once nothing of the tab's is under way, so no rebuild
    // still running takes the Esc first.
    step_until(&mut h, "settled", |h| {
        let tab = &h.state().tabs()[0];
        tab.edits.idle() && !tab.stoppable()
    });
    let title = "Reloaded blocks.silico: another program changed it on disk";
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "dismissed", |h| !anywhere(h, title));
    assert!(h.state().tabs()[0].selection().feature().is_some());
    assert!(!seen(&h, r#""Plate" → "Lid""#));
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(h.state().tabs()[0].selection().is_empty());
    assert!(shows(&h, "Timeline", LID_MARK) && shows(&h, "Browser", LID_MARK));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_reload_lists_features_added_removed_and_moved_each_in_its_words() {
    let f = fixture_reading(Some(Arc::new(real)));
    // The two blocks and a Peg, 4 × 4 × 4 from x = 70 (volume 64, area 6 × 16 = 96, centre
    // (72, 2, 2)).
    let peg = r#"    {"id": "g5r9x3k1", "type": "box", "v": 1, "name": "Peg", "position": [70, 0, 0], "length": 4, "width": 4, "height": 4, "fingerprint": {"volume": 64, "area": 96, "center": [72, 2, 2], "bbox": [70, 0, 0, 74, 4, 4], "faces": 6, "edges": 12, "vertices": 8}}"#;
    let lines: Vec<&str> = BLOCK_AND_PLATE.lines().collect();
    let (block, plate) = (lines[5].trim_end_matches(','), lines[6]);
    let document = |features: [&str; 3]| {
        format!(
            "{}\n{}\n  ]\n}}\n",
            lines[..5].join("\n"),
            features
                .map(|feature| feature.trim_end_matches(','))
                .join(",\n")
        )
    };
    let (mut h, path) = f.opened("blocks.silico", &document([block, plate, peg]));
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Peg"));
    // The Plate moved before the Block, the Peg removed, and a Lid added last, 40 × 20 × 2
    // on the Block (volume 1600, area 2(800 + 80 + 40) = 1840, centre (20, 10, 11)): places
    // by hand, from 1, the Block 1 → 2, the Plate 2 → 1, the Lid at 3, the Peg from 3.
    let lid = r#"    {"id": "q4m7d2e9", "type": "box", "v": 1, "name": "Lid", "position": [0, 0, 10], "length": 40, "width": 20, "height": 2, "fingerprint": {"volume": 1600, "area": 1840, "center": [20, 10, 11], "bbox": [0, 0, 10, 40, 20, 12], "faces": 6, "edges": 12, "vertices": 8}}"#;
    std::fs::write(&path, document([plate, block, lid])).unwrap();
    step_until(&mut h, "the changes listed", |h| seen(h, "at place 3"));
    for label in [
        "moved",
        "place 2 → 1",
        "place 1 → 2",
        "added",
        "at place 3",
        "removed",
        "from place 3",
        "Peg",
    ] {
        assert!(seen(&h, label), "{label}");
    }
    assert_eq!(h.query_all_by_label("moved").count(), 2);
    // Each row its own feature's: the Plate moved up, the Block down, the Lid added, the
    // Peg removed.
    for (text, feature) in [
        ("place 2 → 1", "Plate"),
        ("place 1 → 2", "Block"),
        ("at place 3", "Lid"),
        ("from place 3", "Peg"),
    ] {
        assert_eq!(row_feature(&h, text), feature, "{text}");
    }
    // Each feature still there marked with its words; the Peg, gone, has nothing to mark.
    let marks = [
        "changed on disk: moved place 2 → 1",
        "changed on disk: moved place 1 → 2",
        "changed on disk: added at place 3",
    ];
    step_until(&mut h, "the marks", |h| {
        marks.iter().all(|mark| shows(h, "Timeline", mark))
    });
    assert_eq!(outside_marks(&h, "Timeline"), 3);
    assert_eq!(never_waits::waits(), 0);
}

/// A block, 40 × 20 × 10, in canonical form, with the fingerprint it rebuilds to on the
/// fake kernel: volume 40 × 20 × 10 = 8000, area 2(800 + 400 + 200) = 2800, centre its
/// middle, (20, 10, 5), to z = 10.
const BLOCK_10: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 10,\n      \"fingerprint\": {\"volume\": 8000, \"area\": 2800, \"center\": [20, 10, 5], \"bbox\": [0, 0, 0, 40, 20, 10], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";

/// The agent's change: the block 11 high, its fingerprint volume 40 × 20 × 11 = 8800, area
/// 2(800 + 440 + 220) = 2920, centre (20, 10, 5.5), to z = 11.
const BLOCK_11: &str = "{\n  \"silico\": 1,\n  \"units\": {\"length\": \"mm\", \"resolution\": \"1nm\"},\n  \"naming\": 5,\n  \"features\": [\n    {\n      \"id\": \"k7f2qx3m\",\n      \"type\": \"box\",\n      \"v\": 1,\n      \"name\": \"Block\",\n      \"position\": [0, 0, 0],\n      \"length\": 40,\n      \"width\": 20,\n      \"height\": 11,\n      \"fingerprint\": {\"volume\": 8800, \"area\": 2920, \"center\": [20, 10, 5.5], \"bbox\": [0, 0, 0, 40, 20, 11], \"faces\": 6, \"edges\": 12, \"vertices\": 8}\n    }\n  ]\n}\n";

/// [`BLOCK_10`] `height` high, with the fingerprint it rebuilds to on the fake kernel:
/// volume 40 × 20 × `height`, area 2(800 + 40 `height` + 20 `height`), centre its middle,
/// (20, 10, `height` / 2), to z = `height`.
fn block_high(height: u32) -> String {
    BLOCK_10
        .replace("\"height\": 10", &format!("\"height\": {height}"))
        .replace("\"volume\": 8000", &format!("\"volume\": {}", 800 * height))
        .replace(
            "\"area\": 2800",
            &format!("\"area\": {}", 1600 + 120 * height),
        )
        .replace(
            "[20, 10, 5]",
            &format!("[20, 10, {}]", f64::from(height) / 2.0),
        )
        .replace("40, 20, 10]", &format!("40, 20, {height}]"))
}

/// Whether the session's transaction holds a document other than the one `begin` saw.
fn changed(f: &Fixture, h: &AppHarness) -> bool {
    let id = h.state().tabs()[0].id;
    f.sessions
        .status(&id)
        .result
        .unwrap()
        .transaction
        .is_some_and(|transaction| transaction.changed)
}

#[test]
fn restore_puts_the_replaced_revision_back_as_an_edit_that_undo_takes_and_save_writes() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("block.silico", BLOCK_10);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    std::fs::write(&path, BLOCK_11).unwrap();
    step_until(&mut h, "the changes listed", |h| seen(h, "height"));
    assert!(!changed(&f, &h));
    h.get_by_role_and_label(Role::Button, "Restore revision 1")
        .click();
    step_until(&mut h, "restored", |h| {
        anywhere(h, "block.silico *") && h.state().tabs()[0].edits.idle()
    });
    // The text the reload replaced, byte for byte, as an edit inside a transaction begun
    // for it: unsaved, the banner gone, the file still the agent's.
    assert_eq!(text_of(&f, &h), BLOCK_10);
    assert!(changed(&f, &h));
    let sent = &h.state().tabs()[0].sent;
    assert_eq!(&sent[sent.len() - 2..], [Sent::Begin, Sent::Replace]);
    step_until(&mut h, "unsaved shown", |h| {
        status_said(h, "Unsaved").as_deref() == Some("Unsaved")
    });
    assert!(!anywhere(
        &h,
        "Reloaded block.silico: another program changed it on disk"
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BLOCK_11);
    // Ctrl+Z: the reloaded text back, nothing changed from it.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step_until(&mut h, "undone", |h| text_of(&f, h) == BLOCK_11);
    step_until(&mut h, "unchanged", |h| !changed(&f, h));
    // Redone, then saved: the file is the restored bytes.
    step_until(&mut h, "Redo on", |h| {
        !h.get_by_role_and_label(Role::Button, "Redo")
            .accesskit_node()
            .is_disabled()
    });
    h.get_by_role_and_label(Role::Button, "Redo").click();
    step_until(&mut h, "redone", |h| text_of(&f, h) == BLOCK_10);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "saved", |h| !anywhere(h, "block.silico *"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BLOCK_10);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn restore_is_off_in_sketch_mode_and_on_again_once_it_is_left() {
    // Sketch mode's Cancel would undo a Restore made inside it, and its Finish take it as
    // its own: Restore waits for it to end, as it does for a dialog.
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("block.silico", BLOCK_10);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    std::fs::write(&path, BLOCK_11).unwrap();
    let restore_on = |h: &AppHarness| {
        h.query_by_role_and_label(Role::Button, "Restore revision 1")
            .is_some_and(|button| !button.accesskit_node().is_disabled())
    };
    step_until(&mut h, "the reload's Restore on", |h| restore_on(h));
    create_sketch(&mut h, "XY plane");
    step_until(&mut h, "Restore off in sketch mode", |h| {
        h.query_by_role_and_label(Role::Button, "Restore revision 1")
            .is_some_and(|button| button.accesskit_node().is_disabled())
    });
    // Asked all the same, as a click would: nothing sent.
    let sent = h.state().tabs()[0].sent.len();
    let one = h.state().tabs()[0]
        .reloaded
        .as_ref()
        .map(|reloaded| reloaded.from)
        .unwrap();
    let sessions = Arc::clone(&f.sessions);
    h.state_mut().tabs_mut()[0].restore(&sessions, one);
    assert!(!h.state().tabs()[0].restoring());
    settled(&mut h);
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    leave_sketch(&mut h, false);
    step_until(&mut h, "Restore on again", |h| restore_on(h));
    assert_eq!(text_of(&f, &h), BLOCK_11);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn restore_of_a_revision_no_longer_held_says_the_cores_refusal_and_its_candidates() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("block.silico", BLOCK_10);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    std::fs::write(&path, BLOCK_11).unwrap();
    step_until(&mut h, "the changes listed", |h| seen(h, "height"));
    let id = h.state().tabs()[0].id;
    // The session's queue held, so the banner still offers revision 1, and the file written
    // again, with a read of the document now queued ahead of Restore's (as the disk watch's
    // own reload, or another client's read, would be): it reads the file first, reloading
    // it at revision 3. Restore's read of revision 1 names a revision, so it reads nothing
    // of the file itself.
    let (hold, check) = hold_queue(&f, &h, 0);
    let third = BLOCK_11.replace("\"height\": 11", "\"height\": 12");
    std::fs::write(&path, &third).unwrap();
    let ahead = f.sessions.text(&id, None);
    step_until(&mut h, "the change found", |h| {
        disk(h) == Some(silico_core::disk::Disk::Changed)
    });
    h.get_by_role_and_label(Role::Button, "Restore revision 1")
        .click();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    let ahead = ahead.wait();
    assert_eq!(ahead.session.unwrap().revision.number(), 3);
    // The core's refusal said: revision 1 replaced by the second reload, the session
    // holding 2 (what that reload replaced) and 3 (now).
    let refused = format!(
        "session {id} doesn't hold the text of revision 1: it holds revisions 2 and 3 \
         (candidates: 2, 3)"
    );
    step_until(&mut h, "the refusal said", |h| anywhere(h, &refused));
    // The second reload, which Restore's read came after, announced in the first's place,
    // with what it changed: the height, 11 then 12.
    step_until(&mut h, "the second reload listed", |h| seen(h, "11 → 12"));
    assert!(anywhere(&h, RELOADED.replace("example", "block").as_str()));
    assert!(anywhere(&h, &reloaded_sub(2, 3)));
    assert!(!anywhere(&h, &reloaded_sub(1, 2)));
    assert_eq!(row_feature(&h, "11 → 12"), "Block");
    assert!(!seen(&h, "10 → 11"));
    // Nothing restored: the third text, no transaction; the banner offers revision 2 now.
    assert_eq!(text_of(&f, &h), third);
    assert!(!h.state().tabs()[0].transaction);
    assert!(
        h.query_by_role_and_label(Role::Button, "Restore revision 1")
            .is_none()
    );
    assert!(
        !h.get_by_role_and_label(Role::Button, "Restore revision 2")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(never_waits::waits(), 0);
}

/// When the frame that takes Restore's refusal read the session's `status`, the two orders
/// a frame may see: before the `begin` sent for Restore's edit ran (revision 2, no
/// transaction open, the file found changed), `begin` and `replace_text` then answering
/// before that frame takes its answers; or after `replace_text` had answered (revision 3,
/// the transaction `begin` opened, the file as the session holds it).
#[derive(Debug, Clone, Copy)]
enum StatusRead {
    BeforeBegin,
    AfterReplace,
}

#[test]
fn restore_refused_by_a_reload_before_its_begin_leaves_no_transaction_and_follows_the_file() {
    // Each order forced, so both run every time: the first is the one the frame's status
    // read races `begin` into, which once left no banner.
    for read in [StatusRead::BeforeBegin, StatusRead::AfterReplace] {
        restore_refused_by_a_reload_before_its_begin(read);
    }
}

fn restore_refused_by_a_reload_before_its_begin(read: StatusRead) {
    use silico_core::command::{RunningCommand, SessionCommand};
    let at = format!("{read:?}");
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("block.silico", BLOCK_10);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    std::fs::write(&path, BLOCK_11).unwrap();
    step_until(&mut h, "the changes listed", |h| seen(h, "10 → 11"));
    let id = h.state().tabs()[0].id;
    // Restore's read held behind a check, then let run alone: a second check queued
    // behind it is held, so once that one runs, the read has answered, and the `begin`
    // and `replace_text` the tab sends for it wait behind the second.
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_role_and_label(Role::Button, "Restore revision 1")
        .click();
    step(&mut h);
    hold.release();
    let _ = check.wait();
    let second = f.kernel.hold(|_| true);
    let held = f.sessions.check(&id, CheckModel::Current { fresh: true });
    until("the second check running", || {
        f.sessions.status(&id).result.unwrap().running
            == Some(RunningCommand::Command(SessionCommand::Check))
    });
    step_until(&mut h, "the begin sent", |h| {
        h.state().tabs()[0]
            .sent
            .ends_with(&[Sent::Begin, Sent::Replace])
    });
    // A read queued behind them: once it answers, `begin` and `replace_text` have. Inside
    // the transaction `begin` opens, it reads nothing of the file.
    let behind = f.sessions.text(&id, None);
    // The file written again before `begin` runs: its read reloads it, at revision 3, so
    // the edit, sent against 2, is refused.
    let third = block_high(12);
    std::fs::write(&path, &third).unwrap();
    step_until(&mut h, "the change found", |h| {
        disk(h) == Some(silico_core::disk::Disk::Changed)
    });
    let session = |f: &Fixture| {
        let status = f.sessions.status(&id);
        (
            status.session.unwrap().revision.number(),
            status.result.unwrap().transaction.is_some(),
        )
    };
    match read {
        StatusRead::BeforeBegin => {
            // The next frame's status read, then, before it takes any answer, the second
            // check let go and every command behind it answered.
            let sessions = Arc::clone(&f.sessions);
            let ran = Arc::new(AtomicBool::new(false));
            let told = Arc::clone(&ran);
            let mut gate = Some((second, held, behind));
            h.state_mut().tabs_mut()[0].after_status = Some(Box::new(move || {
                let Some((second, held, mut behind)) = gate.take() else {
                    return;
                };
                let status = sessions.status(&id);
                assert_eq!(status.session.unwrap().revision.number(), 2);
                assert!(status.result.unwrap().transaction.is_none());
                second.release();
                // Polled, not waited on: this is the frame's thread.
                let deadline = Instant::now() + WAIT;
                loop {
                    match behind.try_take() {
                        Ok(_) => break,
                        Err(running) => behind = running,
                    }
                    assert!(Instant::now() < deadline, "never answered behind the edit");
                    std::thread::sleep(Duration::from_millis(1));
                }
                drop(held);
                told.store(true, Ordering::SeqCst);
            }));
            step(&mut h);
            assert!(ran.load(Ordering::SeqCst), "{at}: the hook never ran");
            h.state_mut().tabs_mut()[0].after_status = None;
            // The status that frame read found the file changed: read before `begin`'s
            // read of the file, after which it's as the session holds it.
            assert_eq!(disk(&h), Some(silico_core::disk::Disk::Changed), "{at}");
        }
        StatusRead::AfterReplace => {
            second.release();
            let _ = held.wait();
            let _ = behind.wait();
            // Before any frame: the reload `begin` read, and its transaction open.
            assert_eq!(session(&f), (3, true), "{at}");
            step(&mut h);
            assert_eq!(disk(&h), Some(silico_core::disk::Disk::Unchanged), "{at}");
        }
    }
    // Either way, that frame took the refusal: the reload it came after announced, 2 to 3.
    let banner = h.state().tabs()[0]
        .reloaded
        .as_ref()
        .map(|reloaded| (reloaded.from.number(), reloaded.to.number()));
    assert_eq!(banner, Some((2, 3)), "{at}");
    // The reload announced in the first's place, with what it changed.
    step_until(&mut h, "the second reload listed", |h| seen(h, "11 → 12"));
    assert!(anywhere(&h, &reloaded_sub(2, 3)), "{at}");
    // The transaction `begin` opened for the edit rolled back: nothing held, and the
    // session follows the file again.
    step_until(&mut h, "settled", |h| {
        let tab = &h.state().tabs()[0];
        tab.edits.idle() && !tab.unsettled() && !tab.transaction
    });
    assert!(
        f.sessions.status(&id).result.unwrap().transaction.is_none(),
        "{at}"
    );
    assert_eq!(text_of(&f, &h), third, "{at}");
    assert!(!anywhere(&h, "block.silico *"), "{at}");
    // Another program's next write: reloaded, and announced, from the rollback's revision,
    // 4, to 5.
    let fourth = block_high(13);
    std::fs::write(&path, &fourth).unwrap();
    step_until(&mut h, "the third reload listed", |h| seen(h, "12 → 13"));
    assert!(anywhere(&h, &reloaded_sub(4, 5)), "{at}");
    assert_eq!(text_of(&f, &h), fourth, "{at}");
    assert_eq!(never_waits::waits(), 0, "{at}");
}

/// The session's revision now, as `status` gives it (its file compared on disk as it's
/// asked, so a change found is reloaded), with no frame stepped.
fn revision_now(f: &Fixture, h: &AppHarness) -> u64 {
    let id = h.state().tabs()[0].id;
    f.sessions.status(&id).session.unwrap().revision.number()
}

#[test]
fn a_reload_overtaken_before_its_changes_were_read_is_said_in_the_next_banner() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("block.silico", BLOCK_10);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let id = h.state().tabs()[0].id;
    // Reloaded by the session with no frame stepped; then its queue held, the file written
    // again, and a read of the document now queued, as the disk watch's own reload would be
    // (Fable's residual, *6d's other half*, section 5). Then frames: the tab sees the
    // first reload and asks its diff, 1 → 2, which waits behind that read.
    std::fs::write(&path, BLOCK_11).unwrap();
    until("the first reload", || revision_now(&f, &h) == 2);
    let (hold, check) = hold_queue(&f, &h, 0);
    let third = block_high(12);
    std::fs::write(&path, &third).unwrap();
    let ahead = f.sessions.text(&id, None);
    step_until(&mut h, "the first reload announced", |h| {
        anywhere(h, &reloaded_sub(1, 2))
    });
    // The read ahead reloads the file at 3, so revision 1 is no longer held, and the diff
    // from it is refused, though it reads nothing of the file itself.
    hold.release();
    let _ = check.wait();
    assert_eq!(ahead.wait().session.unwrap().revision.number(), 3);
    step_until(&mut h, "the second reload listed", |h| seen(h, "11 → 12"));
    assert!(anywhere(&h, &reloaded_sub(2, 3)));
    // The first one's changes, never read, said so; its features unmarked.
    assert!(anywhere(
        &h,
        "An earlier change, revision 1 → 2, couldn't be listed: the file changed again \
         before it was read, so its features aren't marked."
    ));
    assert!(!seen(&h, "10 → 11"));
    step_until(&mut h, "the mark", |h| outside_marks(h, "Timeline") == 1);
    let mark = showing(&h, "Timeline", "changed on disk:").unwrap();
    assert!(
        mark.starts_with("changed on disk: height 11 → 12"),
        "{mark}"
    );
    assert!(!mark.contains("10 → 11"), "{mark}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_reloads_diff_names_both_revisions_so_a_write_after_the_reload_never_refuses_it() {
    let reader = Arc::new(HeldReader::default());
    let f = fixture_reading(Some(reader.reader()));
    let (mut h, path) = f.opened("block.silico", BLOCK_10);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let id = h.state().tabs()[0].id;
    // Reloaded by the session with no frame stepped; then its queue held, and frames: the
    // tab sees the reload, 1 → 2, and asks its diff, which waits behind the hold.
    std::fs::write(&path, BLOCK_11).unwrap();
    until("the first reload", || revision_now(&f, &h) == 2);
    let (hold, check) = hold_queue(&f, &h, 0);
    step_until(&mut h, "the first reload announced", |h| {
        anywhere(h, &reloaded_sub(1, 2))
    });
    // The disk watch held, so nothing finds the next write and queues a reload until it's
    // let go; then the file written again, before the diff runs. Asked from 1 to 2 by
    // number, the diff reads nothing of the file; asked to the document now, it would read
    // it first, reload it at 3, and be refused, revision 1 no longer held.
    reader.hold(&mut h);
    std::fs::write(&path, block_high(12)).unwrap();
    hold.release();
    let _ = check.wait();
    // A read queued behind the diff, of a revision too: answered, the diff has, and the
    // session is still at 2.
    let two = Revision::from_number(2).unwrap();
    let behind = f
        .sessions
        .text_at(&id, None, Some(Named::Revision(two)))
        .wait();
    assert_eq!(behind.session.unwrap().revision, two, "the diff reloaded");
    // The first reload's banner lists what it changed, the height 10 then 11, and Restore
    // of revision 1 is on.
    step_until(&mut h, "the first reload listed", |h| seen(h, "10 → 11"));
    // Its rows, the height and the fingerprint written with it: volume 40 × 20 × 10 = 8000
    // then 40 × 20 × 11 = 8800 (and the box's top, z 10 then 11, as the height's).
    assert!(anywhere(&h, &reloaded_sub(1, 2)));
    assert_eq!(h.query_all_by_label("10 → 11").count(), 2);
    assert_eq!(row_feature(&h, "8000 → 8800"), "Block");
    assert!(
        !h.get_by_role_and_label(Role::Button, "Restore revision 1")
            .accesskit_node()
            .is_disabled()
    );
    // The watch let go: it finds the second write, and the session's own reload follows,
    // announced 2 → 3 with what it changed, the first listed, so nothing said unlisted.
    reader.open();
    step_until(&mut h, "the second reload listed", |h| seen(h, "11 → 12"));
    assert!(anywhere(&h, &reloaded_sub(2, 3)));
    assert!(h.query_by_label_contains("couldn't be listed").is_none());
    assert_eq!(text_of(&f, &h), block_high(12));
    assert_eq!(never_waits::waits(), 0);
}

/// [`BLOCK_AND_PLATE`] with only Block renamed, to Base.
fn block_renamed() -> String {
    BLOCK_AND_PLATE.replace(r#""name": "Block""#, r#""name": "Base""#)
}

#[test]
fn the_marks_of_each_reload_accumulate_until_each_is_marked_seen() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("blocks.silico", BLOCK_AND_PLATE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Plate"));
    std::fs::write(&path, block_renamed()).unwrap();
    step_until(&mut h, "the first reload marked", |h| {
        anywhere(h, &reloaded_sub(1, 2)) && shows(h, "Timeline", BASE_MARK)
    });
    assert_eq!(outside_marks(&h, "Timeline"), 1);
    // The second reload changes only the Plate: its banner lists that alone, and the
    // Base's mark, from the first, stays beside the Lid's (the prototype's
    // `Object.assign(d.outside, c.marks)`).
    std::fs::write(&path, blocks_renamed()).unwrap();
    step_until(&mut h, "the second reload marked", |h| {
        anywhere(h, &reloaded_sub(2, 3)) && shows(h, "Timeline", LID_MARK)
    });
    assert!(seen(&h, r#""Plate" → "Lid""#));
    assert!(!seen(&h, r#""Block" → "Base""#));
    assert!(shows(&h, "Timeline", BASE_MARK));
    assert_eq!(outside_marks(&h, "Timeline"), 2);
    step_until(&mut h, "the browser's marks", |h| {
        outside_marks(h, "Browser") == 2
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_diff_answered_after_a_newer_reload_marks_its_features_but_is_never_listed_in_its_banner() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("blocks.silico", BLOCK_AND_PLATE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Plate"));
    let id = h.state().tabs()[0].id;
    // Reloaded with no frame stepped; then one frame, whose poll asks the diff from 1.
    std::fs::write(&path, block_renamed()).unwrap();
    until("the first reload", || revision_now(&f, &h) == 2);
    step(&mut h);
    assert!(anywhere(&h, &reloaded_sub(1, 2)));
    // A read queued behind that diff, waited on off any frame: the diff has answered, its
    // answer there for the tab to take.
    let _ = f.sessions.text(&id, None).wait();
    // Reloaded again, still with no frame stepped: the next frame sees the newer reload
    // with the older diff's answer waiting.
    std::fs::write(&path, blocks_renamed()).unwrap();
    until("the second reload", || revision_now(&f, &h) == 3);
    step_until(&mut h, "the second reload listed", |h| {
        seen(h, r#""Plate" → "Lid""#)
    });
    assert!(anywhere(&h, &reloaded_sub(2, 3)));
    // The older answer, the Block's rename, isn't this banner's.
    assert!(!seen(&h, r#""Block" → "Base""#));
    assert_eq!(row_feature(&h, r#""Plate" → "Lid""#), "Lid");
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(!seen(&h, r#""Block" → "Base""#));
    // But it isn't lost: taken into its own banner before the newer one replaced it, its
    // feature is marked beside the newer one's, and nothing says it couldn't be listed.
    step_until(&mut h, "both marks", |h| {
        shows(h, "Timeline", BASE_MARK) && shows(h, "Timeline", LID_MARK)
    });
    assert_eq!(outside_marks(&h, "Timeline"), 2);
    assert!(h.query_by_label_contains("couldn't be listed").is_none());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn reload_discarding_the_edits_marks_what_came_in_with_no_banner() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("blocks.silico", BLOCK_AND_PLATE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Plate"));
    let changed = "blocks.silico changed on disk while you have unsaved edits.";
    let reloaded = "Reloaded blocks.silico: another program changed it on disk";
    // An edit held, the Plate renamed on disk, and Reload: the person's reload shows no
    // banner (#381), but what came in is marked.
    select(&mut h, "Block");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| h.state().tabs()[0].transaction);
    let lid = BLOCK_AND_PLATE.replace(r#""name": "Plate""#, r#""name": "Lid""#);
    std::fs::write(&path, &lid).unwrap();
    step_until(&mut h, "the change announced", |h| anywhere(h, changed));
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
    step_until(&mut h, "reloaded and marked", |h| {
        shows(h, "Timeline", LID_MARK) && !anywhere(h, "blocks.silico *")
    });
    assert_eq!(outside_marks(&h, "Timeline"), 1);
    for _ in 0..10 {
        step(&mut h);
        assert!(!anywhere(&h, reloaded), "the purple banner after Reload");
    }
    // Again, the reload landing before the tab takes the rollback's answer: no frame
    // stepped from the click's until the session has rolled back and reloaded.
    select(&mut h, "Block");
    type_name(&mut h, "t");
    step_until(&mut h, "the edit held again", |h| {
        h.state().tabs()[0].transaction && h.state().tabs()[0].edits.idle()
    });
    let cap = BLOCK_AND_PLATE.replace(r#""name": "Plate""#, r#""name": "Cap""#);
    std::fs::write(&path, &cap).unwrap();
    step_until(&mut h, "the second change announced", |h| {
        anywhere(h, changed)
    });
    let before = revision_now(&f, &h);
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
    step(&mut h);
    until("rolled back and reloaded", || {
        revision_now(&f, &h) == before + 2
    });
    let capped = r#"changed on disk: name "Lid" → "Cap""#;
    step_until(&mut h, "marked again", |h| shows(h, "Timeline", capped));
    assert_eq!(outside_marks(&h, "Timeline"), 1);
    for _ in 0..10 {
        step(&mut h);
        assert!(
            !anywhere(&h, reloaded),
            "the purple banner after the second Reload"
        );
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn restore_is_off_and_says_why_once_the_reloads_diff_finds_its_revision_gone() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("block.silico", BLOCK_10);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Block"));
    let id = h.state().tabs()[0].id;
    // The first reload seen, its diff held behind a check, and behind a read of the
    // document now queued after the file was written again, as the disk watch's own
    // reload would be.
    std::fs::write(&path, BLOCK_11).unwrap();
    until("the first reload", || revision_now(&f, &h) == 2);
    let (hold, check) = hold_queue(&f, &h, 0);
    std::fs::write(&path, block_high(12)).unwrap();
    let ahead = f.sessions.text(&id, None);
    step_until(&mut h, "the first reload announced", |h| {
        anywhere(h, &reloaded_sub(1, 2))
    });
    // The Block's dialog opened, its read of the feature waiting behind the hold too: the
    // read ahead reloads the file, so the diff from 1 is refused, while the tab's read is
    // under way, so no newer banner takes this one's place.
    double_click(&mut h, "Timeline", "Block");
    hold.release();
    let _ = check.wait();
    assert_eq!(ahead.wait().session.unwrap().revision.number(), 3);
    let refused = format!(
        "session {id} doesn't hold the text of revision 1: it holds revisions 2 and 3 \
         (candidates: 2, 3)"
    );
    step_until(&mut h, "the diff's refusal said, the dialog open", |h| {
        anywhere(h, &refused) && dialog_open(h)
    });
    // The dialog cancelled, having changed nothing: the banner still the first reload's.
    settled(&mut h);
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert!(!h.state().tabs()[0].transaction);
    assert!(anywhere(&h, &reloaded_sub(1, 2)));
    // Restore of revision 1 could only be refused: off, saying why.
    let restore = h.get_by_role_and_label(Role::Button, "Restore revision 1");
    assert!(restore.accesskit_node().is_disabled());
    restore.hover();
    let why = "Silico no longer holds revision 1: the file changed again before what \
               changed was read, so it can't be put back";
    step_until(&mut h, "its tooltip", |h| anywhere(h, why));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn show_changes_lists_what_changed_on_disk_and_the_edits_held_each_in_its_section() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "Pins")
    });
    std::fs::write(
        &path,
        EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Drill""#),
    )
    .unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    // The prototype's order: Show changes, Reload, Keep.
    let buttons = ["Show changes", "Reload, discard my edits", "Keep my edits…"]
        .map(|label| h.get_by_role_and_label(Role::Button, label).rect().left());
    assert!(buttons[0] < buttons[1] && buttons[1] < buttons[2]);
    // Its two diffs held behind a check: each section says it's reading, then lists them.
    let (hold, check) = hold_queue(&f, &h, 0);
    h.get_by_role_and_label(Role::Button, "Show changes")
        .click();
    step_until(&mut h, "the modal", |h| anywhere(h, "What changed on disk"));
    assert_eq!(h.query_all_by_label("Reading what changed…").count(), 2);
    hold.release();
    let _ = check.wait();
    // In the prototype's two columns, each change on one row: the other program's, its
    // feature's name (as the file names it) in the key's, what changed in the value's; the
    // edits held, "Edit" in the key's, named as the document now names it.
    let theirs = r#"name · "Hole" → "Drill""#;
    let edit = r#"Pins · name · "Pin" → "Pins""#;
    step_until(&mut h, "both listed", |h| {
        anywhere(h, theirs) && anywhere(h, edit)
    });
    assert!(!seen(&h, "Reading what changed…"));
    assert_eq!(row_feature(&h, theirs), "Drill");
    assert_eq!(row_feature(&h, edit), "Edit");
    let (key, value) = (h.get_by_label("Edit").rect(), h.get_by_label(edit).rect());
    assert!(key.right() < value.left());
    let (key, value) = (
        h.get_by_label("Drill").rect(),
        h.get_by_label(theirs).rect(),
    );
    assert!(key.right() < value.left());
    let top = |label: &str| h.get_by_label(label).rect().top();
    assert!(top("Their change (on disk)") < top(theirs));
    assert!(top(theirs) < top("Your unsaved edits"));
    assert!(top("Your unsaved edits") < top(edit));
    // Its hint, in words true of Silico: the core's diff, not textconv.
    assert!(anywhere(
        &h,
        "A feature-level summary of the two documents, as Silico's diff reads them."
    ));
    // Closed: the banner still offers the choice.
    h.get_by_role_and_label(Role::Button, "Close").click();
    step_until(&mut h, "closed", |h| !anywhere(h, "What changed on disk"));
    assert!(anywhere(&h, said));
    assert!(h.state().tabs()[0].held_changes.is_none());
    // Opened again, its Reload theirs (the prototype's `reloadDiscard`, as the banner's
    // Reload): the file's text, the edit dropped, the modal and the banner gone.
    h.get_by_role_and_label(Role::Button, "Show changes")
        .click();
    step_until(&mut h, "the modal again", |h| {
        anywhere(h, "What changed on disk")
    });
    for _ in 0..5 {
        step(&mut h);
    }
    let reload = h.get_by_role_and_label(Role::Button, "Reload theirs");
    assert!(!reload.accesskit_node().is_disabled(), "Reload theirs off");
    reload.click();
    step_until(&mut h, "theirs reloaded", |h| {
        shows(h, "Timeline", "Drill")
            && shows(h, "Timeline", "Pin")
            && !anywhere(h, "example.silico *")
    });
    assert!(!shows(&h, "Timeline", "Pins"));
    assert!(!anywhere(&h, "What changed on disk"));
    assert!(!anywhere(&h, said));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn show_changes_lists_their_radius_and_your_height_and_the_revision_never_moves() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", &pinned());
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    // The person's edit, in the Block's dialog: its height 10 → 12, the field focused with
    // its cursor at the end, the "10" erased with Backspace, then "1" and "2" typed.
    double_click(&mut h, "Timeline", "Block");
    step_until(&mut h, "its dialog, loaded", |h| {
        h.query_by_role_and_label(Role::Dialog, "Box").is_some()
            && input(h, "Height").value().as_deref() == Some("10")
    });
    settled(&mut h);
    for _ in 0..10 {
        step(&mut h);
    }
    input(&h, "Height").focus();
    step(&mut h);
    for _ in 0..2 {
        h.key_press(eframe::egui::Key::Backspace);
        step(&mut h);
    }
    assert_eq!(input(&h, "Height").value().as_deref(), Some(""));
    type_into(&mut h, "Height", "12");
    settled(&mut h);
    assert_eq!(input(&h, "Height").value().as_deref(), Some("12"));
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    // An agent's change on disk: the Pin's radius 5 → 3.
    std::fs::write(&path, pinned().replace(r#""radius": 5"#, r#""radius": 3"#)).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    let id = h.state().tabs()[0].id;
    let before = f.sessions.status(&id);
    let revision = before.session.unwrap().revision;
    let transaction = before.result.unwrap().transaction;
    assert!(transaction.is_some());
    h.get_by_role_and_label(Role::Button, "Show changes")
        .click();
    // Fable's two lists (*6d's other half*, section 2): the file's change from the base,
    // and the edits' from the base, each its one change.
    let theirs = "radius · 5 → 3";
    let mine = "Block · height · 10 → 12";
    step_until(&mut h, "both lists", |h| {
        anywhere(h, theirs) && anywhere(h, mine)
    });
    assert_eq!(row_feature(&h, theirs), "Pin");
    assert_eq!(row_feature(&h, mine), "Edit");
    // Each half alone in its list: the edit isn't theirs, nor their change the edits'.
    assert!(!seen(&h, "height · 10 → 12"));
    assert!(!seen(&h, "Pin · radius · 5 → 3"));
    let top = |label: &str| h.get_by_label(label).rect().top();
    assert!(top("Their change (on disk)") < top(theirs));
    assert!(top(theirs) < top("Your unsaved edits"));
    assert!(top("Your unsaved edits") < top(mine));
    // Neither read moved the session: its revision, its transaction's counts, and its
    // finding on disk as they were, the modal still open.
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(anywhere(&h, "What changed on disk"));
    let after = f.sessions.status(&id);
    assert_eq!(after.session.unwrap().revision, revision);
    let after = after.result.unwrap();
    assert_eq!(after.transaction, transaction);
    assert_eq!(after.disk, Some(silico_core::disk::Disk::Changed));
    assert_eq!(h.state().tabs()[0].revision, revision);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn show_changes_says_the_readers_message_for_a_file_that_doesnt_read_in_its_own_section() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "Pins")
    });
    // Another program's file, an object begun and never ended.
    std::fs::write(&path, "{").unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    let revision = h.state().tabs()[0].revision;
    h.get_by_role_and_label(Role::Button, "Show changes")
        .click();
    // Their section: the core's refusal of the side, and under it the reader's message,
    // worked by hand: past the `{`, at line 1, column 2, a key is looked for, and the text
    // has ended. The edits' section answered on its own.
    let refused = "to, the file on disk, doesn't read as a document, so it can't be compared";
    let reader = "the document isn't valid JSON: expected a key, found the end of the text, at \
                  line 1, column 2";
    let edit = r#"Pins · name · "Pin" → "Pins""#;
    step_until(&mut h, "both sections answered", |h| {
        anywhere(h, reader) && anywhere(h, edit)
    });
    assert!(anywhere(&h, refused));
    assert!(!seen(&h, "Reading what changed…"));
    let top = |label: &str| h.get_by_label(label).rect().top();
    assert!(top("Their change (on disk)") < top(refused));
    assert!(top(refused) < top(reader));
    assert!(top(reader) < top("Your unsaved edits"));
    assert!(top("Your unsaved edits") < top(edit));
    // Nothing reloaded: the edit still held.
    assert_eq!(h.state().tabs()[0].revision, revision);
    assert!(anywhere(&h, "example.silico *"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_reload_banner_lists_two_changes_over_the_viewport_with_their_marks() {
    on_lavapipe(
        "the_reload_banner_lists_two_changes_over_the_viewport_with_their_marks",
        || {
            let f = fixture_reading(Some(Arc::new(real)));
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
            h.set_render_every_step(true);
            // The app's fonts, look, and icons, as main.rs sets them, so the shot has words.
            silico_app::theme::apply(&h.ctx);
            let path = f.file("blocks.silico", BLOCK_AND_PLATE);
            h.state_mut().open(path.to_str().unwrap(), false);
            step_until(&mut h, "its features", |h| {
                h.state()
                    .tabs()
                    .first()
                    .is_some_and(|tab| tab.timeline.is_some())
                    && shows(h, "Timeline", "Plate")
            });
            std::fs::write(&path, blocks_renamed()).unwrap();
            step_until(&mut h, "the changes listed and marked", |h| {
                seen(h, r#""Plate" → "Lid""#)
                    && shows(h, "Timeline", BASE_MARK)
                    && shows(h, "Browser", LID_MARK)
            });
            meshes_settled(&mut h);
            for _ in 0..5 {
                step(&mut h);
            }
            // The two rows under the words, and the marks in the timeline and the browser.
            let words = h.get_by_label(&reloaded_sub(1, 2)).rect();
            let base = h.get_by_label(r#""Block" → "Base""#).rect();
            let lid = h.get_by_label(r#""Plate" → "Lid""#).rect();
            assert!(words.bottom() < base.top() && base.bottom() < lid.top());
            assert_eq!(outside_marks(&h, "Timeline"), 2);
            assert_eq!(outside_marks(&h, "Browser"), 2);
            let image = h.render().unwrap();
            if let Some(shot) = std::env::var_os("SILICO_APP_RELOAD_SHOT") {
                image.save(shot).unwrap();
            }
            assert_eq!(never_waits::waits(), 0);
        },
    );
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

/// The session's disk finding, as the tab last read it.
fn disk(h: &AppHarness) -> Option<silico_core::disk::Disk> {
    h.state().tabs()[0]
        .status
        .as_ref()
        .and_then(|status| status.disk)
}

impl HeldReader {
    /// Reads held from now: frames stepped until one is waiting, so no comparison ends,
    /// and the session's finding stands, until [`open`](Self::open). A few frames more, so
    /// the comparison before it, which ended as it began, has told what it found.
    fn hold(&self, h: &mut AppHarness) {
        *self.held.lock().unwrap() = true;
        self.reading.store(false, Ordering::SeqCst);
        step_until(h, "a comparison held", |_| {
            self.reading.load(Ordering::SeqCst)
        });
        for _ in 0..5 {
            step(h);
        }
    }

    fn open(&self) {
        *self.held.lock().unwrap() = false;
        self.opened.notify_all();
    }
}

#[test]
fn a_reload_the_person_asked_for_waits_for_the_file_as_the_session_holds_it() {
    use silico_core::disk::Disk;
    let reader = Arc::new(HeldReader::default());
    let f = fixture_reading(Some(reader.reader()));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    let changed = "example.silico changed on disk while you have unsaved edits.";
    let named = |name: &str| EXAMPLE.replace(r#""name": "Hole""#, &format!(r#""name": "{name}""#));
    // An edit held, and the file changed under it.
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| h.state().tabs()[0].transaction);
    std::fs::write(&path, named("Drill")).unwrap();
    step_until(&mut h, "the change announced", |h| anywhere(h, changed));
    // The comparisons held, Reload, discard my edits: the edits rolled back, the tab settled,
    // and the file still changed on disk as the session last found it. The session's
    // reload, the person's, is still to come.
    reader.hold(&mut h);
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
    step_until(&mut h, "rolled back and settled", |h| {
        let tab = &h.state().tabs()[0];
        !tab.transaction && !tab.unsettled() && shows(h, "Timeline", "Pin")
    });
    for _ in 0..10 {
        step(&mut h);
        assert_eq!(disk(&h), Some(Disk::Changed));
        assert!(shows(&h, "Timeline", "Hole"), "reloaded early");
    }
    // The comparison let go: the session reloads, at the person's asking.
    reader.open();
    step_until(&mut h, "reloaded", |h| shows(h, "Timeline", "Drill"));
    never_reloaded(&mut h, "the person's reload, settled before it came");
    // Again, but the file goes back to what the session holds before it's compared: no
    // reload comes, and the person's is let go once the session finds the file unchanged.
    type_name(&mut h, "t");
    step_until(&mut h, "the edit held again", |h| {
        h.state().tabs()[0].transaction && shows(h, "Timeline", "Pint")
    });
    std::fs::write(&path, named("Bit")).unwrap();
    step_until(&mut h, "the second change announced", |h| {
        anywhere(h, changed)
    });
    reader.hold(&mut h);
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
    step_until(&mut h, "rolled back and settled again", |h| {
        let tab = &h.state().tabs()[0];
        !tab.transaction && !tab.unsettled() && shows(h, "Timeline", "Pin")
    });
    std::fs::write(&path, named("Drill")).unwrap();
    reader.open();
    step_until(&mut h, "found unchanged", |h| {
        disk(h) == Some(Disk::Unchanged)
    });
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(shows(&h, "Timeline", "Drill"));
    // So another program's change after it is announced.
    written_outside(&mut h, &path, "Bore");
    step_until(&mut h, "the reload announced", |h| anywhere(h, RELOADED));
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
    silico_app::theme::apply(&h.ctx);
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
    assert_eq!(names_in(&path), ["Block", "Pin", "HoleS"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_and_close_saves_every_edit_made_before_it_even_one_still_waiting() {
    // "Blockxl", then "Blockxlo" while "Blockxl" waits behind a held command: the save waits
    // for both, so the file holds "Blockxlo", and the close comes after.
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
    // waiting.
    type_name(&mut h, "l");
    assert_eq!(name_field(&h).value().as_deref(), Some("Blockxl"));
    type_name(&mut h, "o");
    assert_eq!(name_field(&h).value().as_deref(), Some("Blockxlo"));
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
    assert_eq!(names_in(&path), ["Blockxlo", "Pin", "Hole"]);
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
        assert_eq!(names_in(path), ["Block", "PinQ", "Hole"]);
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
    assert_eq!(names_in(&b), ["Block", "PinQ", "Hole"]);
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
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "PinA")
    });
    let id = h.state().tabs()[0].id;
    let after_one = f.sessions.text(&id, None).wait().result.unwrap().text;
    type_name(&mut h, "B");
    step_until(&mut h, "two edits", |h| shows(h, "Timeline", "PinAB"));
    h.get_by_role_and_label(Role::Button, "Undo").click();
    step_until(&mut h, "undone", |h| shows(h, "Timeline", "PinA"));
    assert_eq!(
        f.sessions.text(&id, None).wait().result.unwrap().text,
        after_one
    );
    // The field follows the document back (its typed text no longer ahead of it).
    step_until(&mut h, "the field undone", |h| {
        name_field(h).value().as_deref() == Some("PinA")
    });
    h.get_by_role_and_label(Role::Button, "Redo").click();
    step_until(&mut h, "redone", |h| shows(h, "Timeline", "PinAB"));
    step_until(&mut h, "the field redone", |h| {
        name_field(h).value().as_deref() == Some("PinAB")
    });
    // The next keystroke goes after the document's name as it is now.
    type_name(&mut h, "C");
    step_until(&mut h, "the next edit", |h| shows(h, "Timeline", "PinABC"));
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
    assert_eq!(names_in(&path), ["Block", "PinK", "Hole"]);
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
    h.get_by_role_and_label(Role::Button, "Keep my edits…")
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
    // As the prototype's banner for a file gone: no Show changes, there being no file to
    // compare; the edits kept or dropped.
    assert!(
        h.query_by_role_and_label(Role::Button, "Show changes")
            .is_none()
    );
    let buttons = ["Reload, discard my edits", "Keep my edits…"]
        .map(|label| h.get_by_role_and_label(Role::Button, label).rect().left());
    assert!(buttons[0] < buttons[1]);
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
    assert_eq!(names_in(&path), [block.as_str(), "Pinz", "Hole"]);
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
fn new_opens_an_untitled_part_at_once_making_no_file_and_open_cancelled_opens_nothing() {
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
    let before = listing(f.dir.path());
    h.get_by_label("File").click();
    step(&mut h);
    // New asks nothing: no ellipsis, and no question.
    h.get_by_role_and_label(Role::Button, "New").click();
    // Its tab, a button named by the part (the properties panel names it too).
    step_until(&mut h, "its tab", |h| {
        h.query_by_role_and_label(Role::Button, "Untitled")
            .is_some()
    });
    assert!(!anywhere(&h, "New document"));
    assert!(h.query_by_role_and_label(Role::TextInput, "Path").is_none());
    let tab = &h.state().tabs()[0];
    assert_eq!(tab.name(), "Untitled");
    assert_eq!(tab.path, None);
    assert!(!tab.unsaved());
    // The session is the core's untitled one: no path, and its text the empty document.
    assert_eq!(
        f.sessions.status(&tab.id).session.unwrap().path,
        None::<String>
    );
    // Nothing made on disk.
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(listing(f.dir.path()), before);
    assert_eq!(never_waits::waits(), 0);
}

/// The names in `dir`, sorted, and those in its folders, as `folder/name`.
fn listing(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().unwrap().is_dir() {
            names.extend(
                listing(&entry.path())
                    .into_iter()
                    .map(|inner| format!("{name}/{inner}")),
            );
        }
        names.push(name);
    }
    names.sort();
    names
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
    tab.selection_mut()
        .select_feature(silico_core::document::FeatureId::new("a91cw0de").unwrap());
    tab.escape(&f.sessions);
    assert!(tab.selection().is_empty());
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
    // to answer (the cursor at its end, Backspace takes one character each). The refusal is
    // said, and the field keeps "", what the person typed (#298's review).
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    empty_name(&mut h);
    // "Pi", "P", then "" (a name queued behind one in flight is replaced by the next, so
    // "P" may never be sent): "" refused by the gate, its words said in the status bar, the
    // document holding the last name applied before it, "Pi" or "P".
    assert_eq!(
        h.state().tabs()[0].message.as_deref(),
        Some("name is empty (give the feature a name)")
    );
    assert!(anywhere(&h, "name is empty (give the feature a name)"));
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(
        text.contains(r#""name": "P""#) || text.contains(r#""name": "Pi""#),
        "{text}"
    );
    assert_eq!(name_field(&h).value().as_deref(), Some(""));
    assert_eq!(never_waits::waits(), 0);
}

/// Pin's Name emptied with Backspace (the cursor at its end), and "" refused.
fn empty_name(h: &mut AppHarness) {
    name_field(h).focus();
    step(h);
    for _ in 0.."Pin".len() {
        h.key_press(eframe::egui::Key::Backspace);
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
    // Typed at once (one text event).
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
    silico_app::theme::apply(&h.ctx);
    // Laid out again in the app's fonts, so the click lands where Quit is drawn.
    h.step();
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
    assert_eq!(names_in(&path), ["Block", "Pina", "Hole"]);
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
    assert_eq!(names_in(&path), ["Block", "Pins", "Hole"]);
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
    assert_eq!(names_in(&path), ["Block", "Pinq", "Hole"]);
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
    assert!(create_off(&mut h, "Box"));
    let before = name_field(&h).value();
    name_field(&h).type_text("z");
    step(&mut h);
    assert_eq!(name_field(&h).value(), before);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "closed", |h| h.state().tabs().is_empty());
    assert_eq!(names_in(&path), ["Block", "Pinc", "Hole"]);
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

/// Pin renamed "Pinx" and saved, then typed again ("Pinxy"), the field having typed both
/// names.
fn saved_then_typed(h: &mut AppHarness) {
    step_until(h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(h, "Pin");
    type_name(h, "x");
    step_until(h, "Pinx", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "Pinx")
    });
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(h, "saved", |h| {
        h.state().tabs()[0].saves == 1 && !anywhere(h, "example.silico *")
    });
    type_name(h, "y");
    step_until(h, "Pinxy held", |h| {
        anywhere(h, "example.silico *") && shows(h, "Timeline", "Pinxy")
    });
}

#[test]
fn revert_to_a_name_the_field_typed_before_shows_that_name() {
    // The name reverted to is one the field typed ("Pinx", saved), so only the revert's
    // reset, not the name, says the field's "Pinxy" no longer stands.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    saved_then_typed(&mut h);
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step_until(&mut h, "reverted", |h| {
        !anywhere(h, "example.silico *") && shows(h, "Timeline", "Pinx")
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(name_field(&h).value().as_deref(), Some("Pinx"));
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
    assert!(theirs.contains(r#""name": "Pinx""#), "{theirs}");
    std::fs::write(&path, &theirs).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
    step_until(&mut h, "reloaded", |h| {
        shows(h, "Timeline", "Drill") && !anywhere(h, "example.silico *")
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(name_field(&h).value().as_deref(), Some("Pinx"));
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
    assert_eq!(names_in(&path), ["Block", "Pinq", "Hole"]);
    assert!(h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_sessions_own_reload_to_a_name_the_field_typed_before_shows_that_name() {
    // Two saves, "Pinx" then "Pinxy", the field having typed both; then an agent writes
    // "Pinx" back with no transaction open: the session reloads it itself, and the field
    // shows it, not the "Pinxy" it last typed.
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    for (key, name, saves) in [("x", "Pinx", 1), ("y", "Pinxy", 2)] {
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
        .replace(r#""name": "Pinxy""#, r#""name": "Pinx""#);
    assert!(theirs.contains(r#""name": "Pinx""#), "{theirs}");
    std::fs::write(&path, &theirs).unwrap();
    step_until(&mut h, "the agent's name shown", |h| {
        shows(h, "Timeline", "Pinx") && !shows(h, "Timeline", "Pinxy")
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(name_field(&h).value().as_deref(), Some("Pinx"));
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
    assert_eq!(names_in(&path), ["Block", "Pins", "Hole"]);
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
        h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
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
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "Pinx")
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
    while !(anywhere(&h, "example.silico *") && shows(&h, "Timeline", "Pinx")) {
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
        anywhere(h, "example.silico *") && shows(h, "Timeline", "Pinx")
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
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", "Pin\u{1F529}")
    });
    let id = h.state().tabs()[0].id;
    let text = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert!(text.contains("\"name\": \"Pin\u{1F529}\""), "{text}");
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
    // "P" emptied with one Backspace: the only text typed is "", refused; the document's
    // name, "P", unchanged and never typed, so the field keeps "" because the name it
    // shows from is the one it started from.
    let f = fixture();
    let one = EXAMPLE.replace(r#""name": "Pin""#, r#""name": "P""#);
    let (mut h, _) = f.opened("example.silico", &one);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "P"));
    select(&mut h, "P");
    name_field(&h).focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::Backspace);
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
    assert_eq!(h.state().tabs()[0].path.as_deref(), copy.to_str());
    // The new file holds the document as the session holds it, byte for byte; the old one
    // is as it was.
    let id = h.state().tabs()[0].id;
    let held = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), held);
    assert_eq!(names_in(&copy), ["Block", "Pinx", "Hole"]);
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
    // Focused again (its cursor at its end), each character deleted.
    path_field(&h).focus();
    step(&mut h);
    for _ in 0..copy.to_str().unwrap().chars().count() {
        h.key_press(eframe::egui::Key::Backspace);
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
    assert_eq!(h.state().tabs()[0].path.as_deref(), old.to_str());
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
    assert_eq!(names_in(&there), ["Block", "Pinx", "Hole"]);
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
    assert_eq!(h.state().tabs()[0].path.as_deref(), old.to_str());
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
    assert_eq!(names_in(&copy), ["Block", "Pinx", "Hole"]);
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
    assert_eq!(names_in(&old), ["Block", "Pinx", "Hole"]);
    assert_eq!(names_in(&copy), ["Block", "Pinx", "Hole"]);
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
    // Another question asked while the Save As waits (Open's typed path: no native
    // dialogs here).
    h.get_by_label("File").click();
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Open…").click();
    step_until(&mut h, "the Open question", |h| {
        anywhere(h, "Open document")
    });
    hold.release();
    let _ = check.wait();
    let asked = format!("{} already exists. Replace it?", there.to_str().unwrap());
    for _ in 0..20 {
        step(&mut h);
        assert!(
            anywhere(&h, "Open document"),
            "the Open question was replaced"
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

/// The primitives, which the toolbar keeps in CREATE's drop-down alone (app.js:394,
/// interact.js:170).
const PRIMITIVES: [&str; 5] = ["Box", "Cylinder", "Sphere", "Cone", "Torus"];

/// The dialog of `kind` opened from the toolbar's CREATE: a sweep from its row, a
/// primitive from its drop-down.
fn create(h: &mut AppHarness, kind: &str) {
    if PRIMITIVES.contains(&kind) {
        open_create_menu(h);
        h.get_by_role_and_label(Role::Menu, "CREATE")
            .get_by_label(kind)
            .click();
    } else {
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label(kind)
            .click();
    }
    step_until(h, "the dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, kind).is_some()
    });
}

/// CREATE's drop-down opened by a click on its label (`.tb-label`), laid out (egui lays a
/// new popup out unseen in its first frame).
fn open_create_menu(h: &mut AppHarness) {
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_role_and_label(Role::Button, "CREATE")
        .click();
    step_until(h, "CREATE's drop-down", |h| {
        h.query_by_role_and_label(Role::Menu, "CREATE").is_some()
    });
    step(h);
}

/// Whether CREATE's `kind` is off: a sweep in its row; a primitive in its drop-down, which
/// this opens and closes again by a second click on its label.
fn create_off(h: &mut AppHarness, kind: &str) -> bool {
    if !PRIMITIVES.contains(&kind) {
        return h
            .get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label(kind)
            .accesskit_node()
            .is_disabled();
    }
    open_create_menu(h);
    let off = h
        .get_by_role_and_label(Role::Menu, "CREATE")
        .get_by_label(kind)
        .accesskit_node()
        .is_disabled();
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_role_and_label(Role::Button, "CREATE")
        .click();
    step_until(h, "CREATE's drop-down closed", |h| {
        h.query_by_role_and_label(Role::Menu, "CREATE").is_none()
    });
    off
}

/// The dialog's input labelled `label`.
fn input<'a>(h: &'a AppHarness, label: &'a str) -> egui_kittest::Node<'a> {
    dialog(h).get_by_role_and_label(Role::TextInput, label)
}

/// Whether the dialog's choice `key` has `option` chosen: a drop-down's value, or a
/// segmented control's button toggled, as the panel's width draws it.
fn chosen(h: &AppHarness, key: &str, option: &str) -> bool {
    match dialog(h).query_by_role_and_label(Role::ComboBox, key) {
        Some(select) => select.accesskit_node().value().as_deref() == Some(option),
        None => {
            dialog(h)
                .get_by_role_and_label(Role::Button, option)
                .accesskit_node()
                .toggled()
                == Some(eframe::egui::accesskit::Toggled::True)
        }
    }
}

/// `option` chosen in the dialog's choice `key`: from its drop-down's menu, or its
/// segmented control's button, as the panel's width draws it.
fn choose(h: &mut AppHarness, key: &str, option: &str) {
    if dialog(h)
        .query_by_role_and_label(Role::ComboBox, key)
        .is_some()
    {
        dialog(h).get_by_role_and_label(Role::ComboBox, key).click();
        step_until(h, "the drop-down's menu", |h| {
            h.query_all_by_role_and_label(Role::Button, option).count() == 1
        });
        // egui lays a new menu out unseen in its first frame, and places it in its next.
        step(h);
        h.get_by_role_and_label(Role::Button, option).click();
    } else {
        dialog(h)
            .get_by_role_and_label(Role::Button, option)
            .click();
    }
    step(h);
    step(h);
    assert!(chosen(h, key, option), "{option} not chosen in {key}");
}

/// Typed into the dialog's input `label`, one keystroke a frame, at its end.
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

/// Stepped until nothing of the tab's is under way and its last answer's revision is read:
/// its edits, its steps, a dialog's add, and the timeline read at that revision
/// (`applying`), which answers a frame or more after them.
fn settled(h: &mut AppHarness) {
    step_until(h, "everything answered", |h| {
        let tab = &h.state().tabs()[0];
        tab.edits.idle() && !tab.applying() && tab.dialog.as_ref().is_none_or(|d| !d.adding)
    });
}

/// Stepped until the tab's edits, steps, and a dialog's add have answered, the timeline at
/// their revision not waited for: for a test holding the rebuild, whose timeline read
/// waits for the release, so `settled` would never come.
fn answered_while_held(h: &mut AppHarness) {
    step_until(h, "everything but the held timeline answered", |h| {
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
    // Its four options don't fit the panel side by side, so they're a drop-down's.
    choose(&mut h, "Extent", "Two sides");
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
        Plain::of(&json::read(br#"{"type": "two_sides", "length": 5, "length2": 51}"#).unwrap())
    );
    choose(&mut h, "Extent", "Through all");
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
    // The field's text, the 10 and the three digits after it, is the document's.
    let typed = input(&h, "Length").value().unwrap();
    assert_eq!(typed, "10123");
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
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("5x"));
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
    // The minus at the field's start, where a person puts it: Home, then "-".
    input(&h, "Radius").focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::Home);
    step(&mut h);
    input(&h, "Radius").type_text("-");
    step(&mut h);
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
    assert!(before.contains(r#""name": "PinK""#));
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
    assert!(text_of(&f, &h).contains(r#""height": 102"#));
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
        tab.selection_mut().select_feature(marked.unwrap());
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
    assert!(text.contains(r#""width": 103"#), "{text}");
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
    assert!(create_off(&mut h, "Box"));
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
    assert!(after.contains(r#""radius": 51"#) && after.contains(r#""height": 102"#));
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
    // PinK, made before the dialog, stays: one more undo takes it back alone.
    assert!(before.contains(r#""name": "PinK""#));
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
            r#"{"type": "cylinder", "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 123}"#
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
    for kind in ["Pocket", "Groove"] {
        assert!(create_off(&mut h, kind), "{kind}");
    }
    for kind in [
        "Box", "Cylinder", "Sphere", "Cone", "Torus", "Pad", "Revolve",
    ] {
        assert!(!create_off(&mut h, kind), "{kind}");
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
    // The minus at the field's start, where a person puts it: Home, then "-".
    input(&h, "Radius").focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::Home);
    step(&mut h);
    input(&h, "Radius").type_text("-");
    step(&mut h);
    settled(&mut h);
    // "-2.5": a number, sent, and refused by the gate, said in the dialog as the core words it.
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("-2.5"));
    let said = h.state().tabs()[0].message.clone().unwrap();
    assert!(dialog(&h).query_by_label(&said).is_some(), "{said}");
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
    assert!(create_off(&mut h, "Box"));
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
    assert!(chosen(&h, "Axis", "nc000000"));
    assert!(!chosen(&h, "Axis", "Y axis"));
    // And the Y axis offered beside it: a segmented control's button (`chosen` finds it), or
    // an entry in the drop-down's menu, opened and closed again.
    if dialog(&h)
        .query_by_role_and_label(Role::ComboBox, "Axis")
        .is_some()
    {
        dialog(&h)
            .get_by_role_and_label(Role::ComboBox, "Axis")
            .click();
        step_until(&mut h, "the drop-down's menu", |h| {
            h.query_all_by_role_and_label(Role::Button, "Y axis")
                .count()
                == 1
        });
        step(&mut h);
        dialog(&h)
            .get_by_role_and_label(Role::ComboBox, "Axis")
            .click();
        step_until(&mut h, "the drop-down's menu closed", |h| {
            h.query_all_by_role_and_label(Role::Button, "Y axis")
                .count()
                == 0
        });
        assert!(chosen(&h, "Axis", "nc000000"));
    }
    assert!(chosen(&h, "Extent", "Symmetric"));
    assert_eq!(input(&h, "Angle").value().as_deref(), Some("90"));
    assert!(dialog(&h).query_by_label("Triangle").is_some());
    type_into(&mut h, "Angle", ".5");
    settled(&mut h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(
        fields_of(&text_of(&f, &h), "Turned"),
        Some(expected(
            r#"{"type": "revolve", "name": "Turned", "sketch": "s0000001", "axis": "nc000000", "extent": {"type": "symmetric", "angle": 90.5}}"#
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
    assert_eq!(name, "Pinabc");
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    // Undone back to the two letters typed before the dialog.
    let kept = names_of(&text_of(&f, &h))[1].clone();
    assert_eq!(kept, "Pinab");
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
    answered_while_held(&mut h);
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
    answered_while_held(&mut h);
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
    answered_while_held(&mut h);
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
    assert!(text_of(&f, &h).contains(r#""height": 121"#));
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
    // A PlateK typed first, saved: a transaction with something before the dialog is
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
        fields.contains(&("height".to_owned(), Plain::Number("107".to_owned()))),
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
    assert!(named.contains(r#""name": "PinK""#));
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
    h.key_press(eframe::egui::Key::Backspace);
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
    // The minus at the field's start, where a person puts it: Home, then "-".
    input(&h, "Radius").focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::Home);
    step(&mut h);
    input(&h, "Radius").type_text("-");
    step(&mut h);
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
    select(&mut h, "PinK");
    // The queue held: another client's undo (of PinK), then a name typed here against the
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
    // Redo: PinK, then the box's edits whole.
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
    silico_app::theme::apply(&h.ctx);
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
    h.state_mut().tabs_mut()[0]
        .selection_mut()
        .select_feature(silico_core::document::FeatureId::new("a91cw0de").unwrap());
    step(&mut h);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(h.state().tabs()[0].selection().is_empty());
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

/// The export dialog's field `label` emptied (its cursor at its end once focused, a
/// Backspace for each character), then `text` typed at once.
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
        h.key_press(eframe::egui::Key::Backspace);
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
        h.state().tabs()[0].path.as_deref() == folder.join("bare.silico").to_str()
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
        h.state().tabs()[0].path.as_deref() == folder.join(" spaced.silico").to_str()
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
    /// The edge the picking answer gives beside it ([`silico_app::renderer::Picking::edge`]).
    edge: Option<silico_app::renderer::Pick>,
    /// How many of the next frames that ask a pick are to be taken as egui passes that
    /// were discarded: their requests never reach the renderer, and are never answered.
    discard: u32,
    /// The pick asked, and the number of the frame that asked it.
    picking: Option<(u64, silico_app::renderer::PixelRect)>,
    /// If set, what the answer to a pick of the square given finds, in place of `pick`: a
    /// renderer whose answers differ by the pick they're of (a hover's and a click's at
    /// other pixels), however many egui passes a step runs.
    by_rect: Option<
        Box<dyn Fn(silico_app::renderer::PixelRect) -> Option<silico_app::renderer::Pick> + Send>,
    >,
    /// Whether no pick is answered: a renderer that never answers.
    silent: bool,
    /// Whether answers are held back: none is given, and the pick held is the first asked
    /// since, those after it dropped, as a renderer would drop them, until it's let go.
    hold: bool,
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
        if told.hold {
            return None;
        }
        let (frame, rect) = told.picking.take()?;
        let hit = match &told.by_rect {
            Some(by_rect) => by_rect(rect),
            None => told.pick,
        };
        Some(silico_app::renderer::Picking {
            frame,
            rect,
            hit,
            edge: told.edge,
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
                if told.discard > 0 {
                    told.discard -= 1;
                } else if !told.silent && !(told.hold && told.picking.is_some()) {
                    told.picking = Some((frame.serial, rect));
                }
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
        self.drawn_on(name, text, None)
    }

    /// [`Fixture::drawn`], its viewport's time `clock`'s.
    fn drawn_clocked(&self, name: &str, text: &str, clock: Clock) -> (AppHarness, Record) {
        self.drawn_on(name, text, Some(clock))
    }

    fn drawn_on(&self, name: &str, text: &str, clock: Option<Clock>) -> (AppHarness, Record) {
        let path = self.file(name, text);
        let record = Record::default();
        let recorder = Box::new(Recorder(FakeRenderer(Arc::clone(&record))));
        let app = App::new(Arc::clone(&self.sessions));
        let app = match clock {
            None => app.presenting(recorder),
            Some(clock) => app.presenting_clocked(recorder, clock),
        };
        // A sixtieth of a second a frame, as a display's: a double-click's two clicks are
        // two frames apart.
        let mut h = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size((1280.0, 800.0))
                .with_step_dt(1.0 / 60.0)
                .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
        };
        silico_app::theme::apply(&h.ctx);
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
            hit: Some(pick),
            edge: None,
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
    // From (25, −d, 25) onto (25, 25, 25) at 30°: both cubes' nearest faces are at y = 0,
    // d deep, so each half pixel is d · tan 15° / H, for the frame's H. With d set so that's
    // 0.24, the small cube is meshed at the power of two below, 0.125; the large one's
    // floor, 0.1732, is coarser than that level, so it's meshed at its floor, serving
    // [0, 0.25). The target is 25 deeper, past 0.25: a body read at the target's depth
    // would be meshed a level coarser.
    let f = fixture();
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    let height = f64::from(view(&h).frame().unwrap().size_px[1]);
    let tan = libm::tan(15f64.to_radians());
    let depth = 0.24 * height / tan;
    assert!(
        (depth + 25.0) * tan / height >= 0.25,
        "the frame, {height} high, puts the target's half pixel under 0.25"
    );
    view_mut(&mut h).set_camera(silico_protocol::Camera {
        projection: silico_protocol::Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: [25.0, -depth, 25.0],
        target: [25.0; 3],
        up: [0.0, 0.0, 1.0],
    });
    meshes_settled(&mut h);
    assert_eq!(f64::from(view(&h).frame().unwrap().size_px[1]), height);
    let level = 0.125;
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
            silico_app::theme::apply(&h.ctx);
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
                    hovered: Vec::new(),
                    outlines: silico_app::renderer::Outlines::DEFAULT,
                    picking: None,
                    vertices: None,
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
    silico_app::theme::apply(&h.ctx);
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
        silico_app::theme::apply(&h.ctx);
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

// ---- Native file dialogs: Open's panel, and Browse… beside each path field ----
//
// The platform's own dialog (rfd's, `browse::Native`) needs a window server and a person, so
// it can't run headless: these drive a stand-in through the same `Browse` trait, which
// records what each dialog was asked and answers when the test says, from a thread of its
// own, as the platform's completion does; or, told to, opens none, as when the platform's
// won't. Open goes straight to the panel; its typed-path question, which has a Browse… of
// its own, is the fallback for a panel that won't open.

use silico_app::browse::{self as browse_mod, Ask, Browse, Pick};

/// The stand-in's state: what each dialog was asked, the answer the test gave (`None` until
/// it does), the waker of the last poll, how many polls there were, and whether a dialog
/// asked for opens none.
#[derive(Default)]
struct Dialogs {
    asked: Vec<Ask>,
    answer: Option<Option<PathBuf>>,
    waker: Option<std::task::Waker>,
    polls: u64,
    refusing: bool,
}

#[derive(Clone, Default)]
struct FakeBrowser(Arc<Mutex<Dialogs>>);

impl Browse for FakeBrowser {
    fn browse(&mut self, ask: &Ask) -> Option<browse_mod::Answer> {
        let mut dialogs = self.0.lock().unwrap();
        dialogs.asked.push(ask.clone());
        if dialogs.refusing {
            return None;
        }
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

    /// Each dialog asked for from now on opens none (`refusing`), or opens.
    fn refusing(&self, refusing: bool) {
        self.0.lock().unwrap().refusing = refusing;
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
        let h = Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app);
        silico_app::theme::apply(&h.ctx);
        h
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

/// File > `item` clicked.
fn file_menu_item(h: &mut AppHarness, item: &str) {
    h.get_by_label("File").click();
    step(h);
    step(h);
    button_off_start(h, item).click();
}

/// Frames stepped until the native dialog has been asked for a `nth` time (from 1), none of
/// them showing the typed-path Open: Open's panel asked at once, with no question first.
#[track_caller]
fn open_panel_asked(h: &mut AppHarness, browser: &FakeBrowser, nth: usize) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(h);
        assert!(!anywhere(h, "Open document"), "the typed-path Open shown");
        if browser.asked().len() >= nth {
            break;
        }
        assert!(Instant::now() < deadline, "the open panel never asked");
    }
    assert_eq!(browser.asked().len(), nth);
    assert_eq!(browser.asked()[nth - 1].pick, Pick::Open);
    assert_eq!(browser.asked()[nth - 1].title, "Open document");
}

/// File > Open…, with the panel refusing to open: the typed-path Open, its fallback, shown;
/// the panel opening again afterwards.
fn typed_open(h: &mut AppHarness, browser: &FakeBrowser) {
    browser.refusing(true);
    file_menu(h, "Open…", "Open document");
    browser.refusing(false);
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
fn opens_panel_asks_for_a_silico_file_frames_climb_while_it_is_open_and_its_pick_opens() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 1);
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
    // While it's open: frames climb, each polling it, never waiting; Open again asks no
    // second panel over it, nor the typed-path question.
    let (frames, polls) = (h.state().frames(), browser.polls());
    for _ in 0..50 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), frames + 50);
    assert!(browser.polls() >= polls + 50, "{}", browser.polls());
    file_menu_item(&mut h, "Open…");
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::O);
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    assert!(!anywhere(&h, "Open document"));
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
    assert_eq!(h.state().tabs()[0].path.as_deref(), path.to_str());
    assert!(!anywhere(&h, "Open document"));
    // Open again: the shown document's folder is where it starts.
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 2);
    assert_eq!(browser.asked()[1].folder.as_deref(), Some(f.dir.path()));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_typed_path_opens_browse_asks_for_a_silico_file_and_its_pick_opens_as_typed() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    // The panel wouldn't open: Open asked it, then asked for a typed path.
    typed_open(&mut h, &browser);
    assert_eq!(browser.asked().len(), 1);
    // A path typed first, which the pick's open takes as a typed one's would.
    h.get_by_role(Role::TextInput).focus();
    step(&mut h);
    h.get_by_role(Role::TextInput).type_text("typed.silico");
    step(&mut h);
    browse(&mut h, &browser);
    assert_eq!(browser.polls(), 1);
    assert_eq!(browser.asked()[0], browser.asked()[1]);
    assert_eq!(browser.asked()[1].pick, Pick::Open);
    // While it's open, Browse… is off, so no second dialog opens over it.
    step(&mut h);
    assert!(browse_button(&h).accesskit_node().is_disabled());
    browse_button(&h).click_accesskit();
    step(&mut h);
    assert_eq!(browser.asked().len(), 2);
    assert!(h.state().tabs().is_empty());
    let path = f.file("picked.silico", EXAMPLE);
    browser.answer(Some(&path));
    step_until(&mut h, "its tab", |h| anywhere(h, "picked.silico"));
    assert_eq!(h.state().tabs()[0].path.as_deref(), path.to_str());
    assert!(!anywhere(&h, "Open document"));
    // Open again, the panel refusing still: its field emptied.
    typed_open(&mut h, &browser);
    assert_eq!(path_value(&h), "");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancelled_browse_changes_nothing() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, path) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    // Open's panel cancelled: nothing opens, nothing is asked or said.
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 1);
    browser.answer(None);
    for _ in 0..10 {
        step(&mut h);
        assert!(!anywhere(&h, "Open document"));
    }
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(h.state().message(), None);
    // And it can be asked again.
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 2);
    browser.answer(None);
    // The typed-path Open, a path typed first: cancelled, the field holds what was typed and
    // nothing opens.
    typed_open(&mut h, &browser);
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
    // Save As: its Save panel at once, cancelled: nothing is saved, and no question is
    // asked after it.
    let asked = browser.asked().len();
    file_menu_item(&mut h, "Save As…");
    step_until(&mut h, "the Save panel asked", |_| {
        browser.asked().len() > asked
    });
    assert_eq!(browser.asked()[asked].pick, Pick::Save);
    browser.answer(None);
    for _ in 0..10 {
        step(&mut h);
        assert!(!anywhere(&h, "Save example.silico as"));
    }
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert_eq!(h.state().tabs()[0].path.as_deref(), path.to_str());
    assert_eq!(h.state().message(), None);
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

/// Frames stepped until the native dialog has been asked for an `nth` time (from 1), none
/// of them showing the typed Save As: the Save panel asked at once, with no question first.
#[track_caller]
fn save_panel_asked(h: &mut AppHarness, browser: &FakeBrowser, nth: usize) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(h);
        assert!(
            h.query_by_role_and_label(Role::TextInput, "Path").is_none(),
            "a typed path asked for"
        );
        if browser.asked().len() >= nth {
            break;
        }
        assert!(Instant::now() < deadline, "the Save panel never asked");
    }
    assert_eq!(browser.asked().len(), nth);
    assert_eq!(browser.asked()[nth - 1].pick, Pick::Save);
}

#[test]
fn save_as_asks_the_save_panel_and_its_pick_is_saved_replacing_a_file_there() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, old) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    file_menu_item(&mut h, "Save As…");
    save_panel_asked(&mut h, &browser, 1);
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
    // While it's open: frames climb, nothing saved; Save As again asks no second panel.
    let frames = h.state().frames();
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(h.state().frames(), frames + 20);
    assert_eq!(h.state().tabs()[0].saves, 0);
    file_menu_item(&mut h, "Save As…");
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    // Picked: saved as it at once, the tab following the new file.
    let copy = f.dir.path().join("copy.silico");
    browser.answer(Some(&copy));
    step_until(&mut h, "the tab of the new file", |h| {
        h.state().tabs()[0].path.as_deref() == copy.to_str()
    });
    assert_eq!(h.state().tabs()[0].saves, 1);
    // The document as the session holds it, byte for byte; the old file as it was.
    let id = h.state().tabs()[0].id;
    let held = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), held);
    assert_eq!(names_in(&copy), ["Block", "Pin", "Hole"]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), EXAMPLE);
    // A file already there, picked: the panel asked about replacing it, so it's replaced,
    // with no question of the app's.
    let there = f.file("there.silico", "an older copy");
    step_until(&mut h, "saved", |h| !anywhere(h, "copy.silico *"));
    file_menu_item(&mut h, "Save As…");
    save_panel_asked(&mut h, &browser, 2);
    // Now the new file's folder and name.
    assert_eq!(browser.asked()[1].file_name.as_deref(), Some("copy.silico"));
    assert_eq!(browser.asked()[1].folder.as_deref(), Some(f.dir.path()));
    browser.answer(Some(&there));
    step_until(&mut h, "the tab of the file replaced", |h| {
        h.state().tabs()[0].path.as_deref() == there.to_str()
    });
    let asked = format!("{} already exists. Replace it?", there.to_str().unwrap());
    assert!(!anywhere(&h, &asked));
    assert_eq!(std::fs::read_to_string(&there).unwrap(), held);
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
fn an_untitled_parts_save_panel_asks_its_name_and_the_next_starts_in_the_folder_picked() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    // New asks nothing, not even the panel: an untitled part at once.
    file_menu_item(&mut h, "New");
    step_until(&mut h, "its tab", |h| h.state().tabs().len() == 1);
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(browser.asked().is_empty());
    // Save As: the panel, named by the part, starting where the platform says (no folder
    // picked yet, and the part has none).
    file_menu_item(&mut h, "Save As…");
    save_panel_asked(&mut h, &browser, 1);
    assert_eq!(
        browser.asked(),
        [Ask {
            pick: Pick::Save,
            title: "Save Untitled as".to_owned(),
            filters: vec![browse_mod::DOCUMENTS],
            folder: None,
            file_name: Some("Untitled.silico".to_owned()),
        }]
    );
    let sub = f.dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let made = sub.join("new.silico");
    browser.answer(Some(&made));
    step_until(&mut h, "the tab of the new file", |h| {
        h.state().tabs()[0].path.as_deref() == made.to_str()
    });
    assert!(made.is_file());
    assert_eq!(h.state().tabs()[0].name(), "new.silico");
    // Closed: no document open, the next starts in the folder last picked in.
    h.get_by_label("Close new.silico").click();
    step_until(&mut h, "it closed", |h| h.state().tabs().is_empty());
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 2);
    assert_eq!(browser.asked()[1].folder.as_deref(), Some(sub.as_path()));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_pick_answering_after_its_dialog_closed_changes_nothing() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    typed_open(&mut h, &browser);
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
    typed_open(&mut h, &browser);
    assert_eq!(path_value(&h), "");
    assert!(!browse_button(&h).accesskit_node().is_disabled());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_opens_pick_answering_while_save_as_is_asked_neither_opens_nor_fills_it() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, path) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    // The typed-path Open, a path typed, browsing; then cancelled, its native dialog still
    // open.
    typed_open(&mut h, &browser);
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
    assert_eq!(h.state().tabs()[0].path.as_deref(), path.to_str());
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
    typed_open(&mut h, &browser);
    assert_eq!(path_value(&h), "typed.silico");
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn open_panels_pick_answering_while_save_as_is_asked_opens_it_and_leaves_save_as_alone() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, path) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    // Open's panel open (the portal's, on Linux, leaves the window to be used meanwhile).
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 1);
    file_menu(&mut h, "Save As…", "Save example.silico as");
    let picked = f.file("picked.silico", EXAMPLE);
    browser.answer(Some(&picked));
    step_until(&mut h, "its tab", |h| h.state().tabs().len() == 2);
    assert_eq!(h.state().tabs()[1].path.as_deref(), picked.to_str());
    // Save As still asked of the first document, its field as it was.
    assert!(anywhere(&h, "Save example.silico as"));
    assert_eq!(path_value(&h), "");
    assert_eq!(h.state().tabs()[0].path.as_deref(), path.to_str());
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_exports_pick_answering_while_open_is_asked_neither_fills_it_nor_exports() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let (mut h, _) = f.opened_browsing(&browser, "example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    open_export(&mut h);
    let before = path_value(&h);
    browse(&mut h, &browser);
    press(&mut h, "Cancel");
    step_until(&mut h, "the dialog closed", |h| !exporting_dialog(h));
    // The typed-path Open asked (Export's dialog still open, so not its panel), and only
    // then Export's pick answers, a STEP name.
    file_menu(&mut h, "Open…", "Open document");
    let late = f.dir.path().join("late.step");
    browser.answer(Some(&late));
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(anywhere(&h, "Open document"));
    assert_eq!(path_value(&h), "");
    assert!(!browse_button(&h).accesskit_node().is_disabled());
    assert_eq!(h.state().tabs().len(), 1);
    assert!(h.state().tabs()[0].exports.is_empty());
    assert!(!late.exists());
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "the dialog gone", |h| !anywhere(h, "Open document"));
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
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 1);
    browser.answer(None);
    step(&mut h);
    // No document open and nothing picked: the platform's own start again.
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 2);
    assert_eq!(browser.asked()[1].folder, None);
    // So too through Browse… (the typed-path Open's, its panel refused: a third asked).
    browser.answer(None);
    step(&mut h);
    typed_open(&mut h, &browser);
    browse(&mut h, &browser);
    browser.answer(None);
    step_until(&mut h, "Browse… on again", |h| {
        !browse_button(h).accesskit_node().is_disabled()
    });
    browse(&mut h, &browser);
    assert_eq!(browser.asked().len(), 5);
    assert_eq!(browser.asked()[4].folder, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn browse_while_a_dialog_is_open_is_off_and_says_why_when_hovered() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    typed_open(&mut h, &browser);
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
    let mut bytes = f.dir.path().as_os_str().as_bytes().to_vec();
    bytes.extend_from_slice(b"/\xff.silico");
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
    let said = format!("{} can't be used: its path isn't UTF-8", path.display());
    // Open's panel: said, nothing opened, nothing asked.
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 1);
    browser.answer(Some(&path));
    step_until(&mut h, "said", |h| h.state().message().is_some());
    assert_eq!(h.state().message(), Some(said.as_str()));
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(h.state().tabs().is_empty());
    assert!(!anywhere(&h, "Open document"));
    // The typed-path Open's Browse…, another such path picked: said, its field left empty,
    // and it still asked.
    let mut bytes = f.dir.path().as_os_str().as_bytes().to_vec();
    bytes.extend_from_slice(b"/\xfe.silico");
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
    let said = format!("{} can't be used: its path isn't UTF-8", path.display());
    typed_open(&mut h, &browser);
    browse(&mut h, &browser);
    browser.answer(Some(&path));
    step_until(&mut h, "said", |h| {
        h.state().message() == Some(said.as_str())
    });
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(h.state().message(), Some(said.as_str()));
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
    // File > Open…: Open's panel asked at once, no question of the app's first.
    file_menu_item(&mut h, "Open…");
    open_panel_asked(&mut h, &browser, 1);
    quiet_wakes(&mut h);
    idle(&mut h);
    let path = f.file("picked.silico", EXAMPLE);
    browser.answer(Some(&path));
    let woken = h.state().wakes();
    // The frame the answer's waker asked for (egui gives each such request two frames): it
    // takes the pick, asking no question, and the open is refused, not yet taken.
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

/// How Open is asked for: Command+O, File > Open…, or the start page's Open….
const OPEN_ROUTES: [&str; 3] = ["Command+O", "File > Open…", "the start page's Open…"];

/// Open asked for by `route`, one of [`OPEN_ROUTES`], from the start page.
fn ask_open_by(h: &mut AppHarness, route: &str) {
    match route {
        "Command+O" => {
            h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::O);
        }
        "File > Open…" => file_menu_item(h, "Open…"),
        _ => start_page(h)
            .get_by_role_and_label(Role::Button, "Open…")
            .click(),
    }
}

#[test]
fn each_way_to_open_asks_the_panel_at_once_and_a_cancel_shows_nothing() {
    let f = fixture();
    for route in OPEN_ROUTES {
        let browser = FakeBrowser::default();
        let mut h = f.app_browsing(&browser);
        step(&mut h);
        assert!(start_shown(&h));
        ask_open_by(&mut h, route);
        if route == "Command+O" {
            // The frame that takes the key asks it.
            step(&mut h);
            assert_eq!(browser.asked().len(), 1, "{route}");
        }
        open_panel_asked(&mut h, &browser, 1);
        assert_eq!(
            browser.asked()[0].filters,
            [browse_mod::DOCUMENTS],
            "{route}"
        );
        // Cancelled: nothing shown, opened, or said, and the start page stays.
        browser.answer(None);
        for _ in 0..10 {
            step(&mut h);
            assert!(!anywhere(&h, "Open document"), "{route}");
        }
        assert!(h.state().tabs().is_empty(), "{route}");
        assert_eq!(h.state().message(), None, "{route}");
        assert_eq!(browser.asked().len(), 1, "{route}");
        assert!(start_shown(&h), "{route}");
        assert_eq!(never_waits::waits(), 0);
    }
}

#[test]
fn each_way_to_open_opens_the_panels_pick() {
    let f = fixture();
    for (at, route) in OPEN_ROUTES.into_iter().enumerate() {
        let browser = FakeBrowser::default();
        let mut h = f.app_browsing(&browser);
        step(&mut h);
        ask_open_by(&mut h, route);
        open_panel_asked(&mut h, &browser, 1);
        let path = f.file(&format!("picked{at}.silico"), EXAMPLE);
        browser.answer(Some(&path));
        step_until(&mut h, "its tab", |h| !h.state().tabs().is_empty());
        assert_eq!(h.state().tabs().len(), 1, "{route}");
        assert_eq!(
            h.state().tabs()[0].path.as_deref(),
            path.to_str(),
            "{route}"
        );
        step_until(&mut h, "the start page left", |h| !start_shown(h));
        assert!(!anywhere(&h, "Open document"), "{route}");
        assert_eq!(never_waits::waits(), 0);
    }
}

#[test]
fn a_panel_that_wont_open_asks_for_a_typed_path_which_opens() {
    let f = fixture();
    for (at, route) in OPEN_ROUTES.into_iter().enumerate() {
        let browser = FakeBrowser::default();
        browser.refusing(true);
        let mut h = f.app_browsing(&browser);
        step(&mut h);
        ask_open_by(&mut h, route);
        step_until(&mut h, "the typed-path Open", |h| {
            anywhere(h, "Open document")
        });
        // The panel was asked for first.
        assert_eq!(browser.asked().len(), 1, "{route}");
        assert_eq!(browser.asked()[0].pick, Pick::Open, "{route}");
        let path = f.file(&format!("typed{at}.silico"), EXAMPLE);
        h.get_by_role_and_label(Role::TextInput, "Path").focus();
        step(&mut h);
        h.get_by_role_and_label(Role::TextInput, "Path")
            .type_text(path.to_str().unwrap());
        step(&mut h);
        h.key_press(eframe::egui::Key::Enter);
        step_until(&mut h, "its tab", |h| !h.state().tabs().is_empty());
        assert_eq!(
            h.state().tabs()[0].path.as_deref(),
            path.to_str(),
            "{route}"
        );
        step_until(&mut h, "the question gone", |h| {
            !anywhere(h, "Open document")
        });
        assert_eq!(browser.asked().len(), 1, "{route}");
        assert_eq!(never_waits::waits(), 0);
    }
}

#[test]
fn with_no_native_dialogs_each_way_to_open_asks_for_a_typed_path() {
    let f = fixture();
    for route in OPEN_ROUTES {
        let mut h = f.app();
        step(&mut h);
        ask_open_by(&mut h, route);
        step_until(&mut h, "the typed-path Open", |h| {
            anywhere(h, "Open document")
        });
        assert!(
            h.query_by_role_and_label(Role::TextInput, "Path").is_some(),
            "{route}"
        );
        h.key_press(eframe::egui::Key::Escape);
        step_until(&mut h, "it gone", |h| !anywhere(h, "Open document"));
        assert!(h.state().tabs().is_empty(), "{route}");
        assert_eq!(never_waits::waits(), 0);
    }
}

/// Whether `pixel` is within `by` of `rgb` in each channel.
fn pixel_near(pixel: [u8; 4], rgb: [u8; 3], by: u8) -> bool {
    pixel[..3]
        .iter()
        .zip(rgb)
        .all(|(&have, want)| have.abs_diff(want) <= by)
}

/// A field's frame as `pixel` reads a frame drawn: the pixels of the two columns its left
/// edge crosses, at its mid-height (its 1-point border, antialiased over them).
fn left_edge(
    pixel: &dyn Fn(eframe::egui::Pos2) -> [u8; 4],
    field: eframe::egui::Rect,
) -> [[u8; 4]; 2] {
    let y = field.center().y;
    [
        pixel(eframe::egui::pos2(field.left(), y)),
        pixel(eframe::egui::pos2(field.left() + 1.0, y)),
    ]
}

#[test]
fn text_fields_are_drawn_as_the_prototypes_in_each_state_on_lavapipe() {
    on_lavapipe(
        "text_fields_are_drawn_as_the_prototypes_in_each_state_on_lavapipe",
        || {
            let f = fixture();
            let state = lavapipe_render_state();
            let app = App::new(Arc::clone(&f.sessions));
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
            step(&mut h);
            let ppp = h.ctx.pixels_per_point();
            let (blue, red, gray_350) =
                ([0x46, 0x76, 0xff], [0xff, 0x38, 0x65], [0x52, 0x51, 0x51]);

            // A modal's field. No native dialogs: Open asks for a typed path, in a modal.
            h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::O);
            step_until(&mut h, "the typed-path Open", |h| {
                anywhere(h, "Open document")
            });
            // Past the modal's fade-in (egui's animation time, a twelfth of a second).
            for _ in 0..30 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            let field = h.get_by_role_and_label(Role::TextInput, "Path").rect();
            assert!(field.width() > 40.0 && field.height() > 10.0, "{field:?}");
            // Empty, unfocused, unhovered: inside, its fill, Gray.150 (styles.css,
            // `.modify-box input`, the prototype's field on a floating surface).
            assert_eq!(pixel(field.center()), [0x17, 0x17, 0x17, 0xff], "{field:?}");
            assert_eq!(
                pixel(field.right_center() + eframe::egui::vec2(-4.0, 0.0)),
                [0x17, 0x17, 0x17, 0xff],
                "{field:?}"
            );
            // Just outside it, above: the modal's own fill, Gray.200 (`.modal`), which the
            // field's was, so it showed nothing.
            assert_eq!(
                pixel(field.center_top() + eframe::egui::vec2(0.0, -3.0)),
                [0x21, 0x21, 0x21, 0xff],
                "{field:?}"
            );
            // Its border: Gray.350's (antialiased, so near it, not exactly).
            let edge = left_edge(&pixel, field);
            assert!(
                edge.iter().any(|&px| pixel_near(px, gray_350, 24)),
                "{edge:?}"
            );
            // Focused: the focus blue's border (`.modify-box input:focus`), the only focus
            // ring a framed field has (egui draws none of its own on one).
            h.get_by_role_and_label(Role::TextInput, "Path").focus();
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            let edge = left_edge(&pixel, field);
            assert!(edge.iter().any(|&px| pixel_near(px, blue, 48)), "{edge:?}");
            assert!(
                !edge.iter().any(|&px| pixel_near(px, gray_350, 24)),
                "{edge:?}"
            );
            h.key_press(eframe::egui::Key::Escape);
            step_until(&mut h, "it gone", |h| !anywhere(h, "Open document"));

            // A panel's field: the Sphere's dialog's Radius, docked at the right.
            let path = f.file("example.silico", EXAMPLE);
            h.state_mut().open(path.to_str().unwrap(), false);
            step_until(&mut h, "the timeline read", |h| {
                h.state()
                    .tabs()
                    .first()
                    .is_some_and(|tab| tab.timeline.is_some())
            });
            step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
            create(&mut h, "Sphere");
            added(&mut h);
            settled(&mut h);
            // The pointer off it, on the dialog's title.
            dialog(&h).get_by_label("SPHERE").hover();
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            let field = input(&h, "Radius").rect();
            assert!(field.width() > 30.0 && field.height() > 10.0, "{field:?}");
            // Inside it, clear of its text (at its left) and its border.
            let inside = field.right_center() + eframe::egui::vec2(-4.0, 0.0);
            // At rest: its fill Gray.200 (`.field`), on the panel's Gray.100 just above it
            // (a 22-high field in a 24-high `.prop` row: a point clear of it, between it and
            // the field of the row above).
            assert_eq!(pixel(inside), [0x21, 0x21, 0x21, 0xff], "{field:?}");
            assert_eq!(
                pixel(field.center_top() + eframe::egui::vec2(0.0, -1.0)),
                [0x0d, 0x0d, 0x0d, 0xff],
                "{field:?}"
            );
            // Hovered: Gray.250 (`.field:hover`).
            input(&h, "Radius").hover();
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            assert_eq!(pixel(inside), [0x27, 0x26, 0x26, 0xff], "{field:?}");
            // A value that doesn't parse: the red border (`.field.bad`), focused or not.
            type_into(&mut h, "Radius", "x");
            for _ in 0..5 {
                step(&mut h);
            }
            // Its 5 and the x, in either order (where the caret was), parse as no number.
            let typed = input(&h, "Radius").value().unwrap_or_default();
            assert!(["x5", "5x"].contains(&typed.as_str()), "{typed}");
            let image = h.render().unwrap();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            let edge = left_edge(&pixel, field);
            assert!(edge.iter().any(|&px| pixel_near(px, red, 48)), "{edge:?}");
            assert!(!edge.iter().any(|&px| pixel_near(px, blue, 48)), "{edge:?}");
        },
    );
}

#[test]
fn open_while_a_browse_is_open_asks_for_a_typed_path_and_opens_no_second_dialog() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    // The typed Open's Browse… (its panel refused), its native dialog left open, and that
    // Open cancelled.
    typed_open(&mut h, &browser);
    browse(&mut h, &browser);
    h.get_by_role_and_label(Role::Button, "Cancel")
        .click_accesskit();
    step_until(&mut h, "Open gone", |h| !anywhere(h, "Open document"));
    let asked = browser.asked().len();
    // Open: the typed-path question, the platform's dialog not asked for again.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::O);
    step_until(&mut h, "the typed-path Open", |h| {
        anywhere(h, "Open document")
    });
    assert_eq!(browser.asked().len(), asked);
    // Its Browse… is off while the other dialog is open.
    assert!(browse_button(&h).accesskit_node().is_disabled());
    assert_eq!(never_waits::waits(), 0);
}

// ---- The start page (docs/design/m5-ui/DESIGN.md, *Start page or blank canvas?*) ----

impl Fixture {
    /// The app on these sessions as `make` makes it, in a harness, with nothing open.
    fn app_made(&self, make: impl FnOnce(App<Fake>) -> App<Fake>) -> AppHarness {
        let app = make(App::new(Arc::clone(&self.sessions)));
        let _marked = never_waits::marked();
        let h = Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app);
        silico_app::theme::apply(&h.ctx);
        h
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
    // New part opens an untitled part at once, as File > New does, asking nothing, and
    // the start page gives way to it; listed in Recent only once it's saved.
    start_page(&h)
        .get_by_role_and_label(Role::Button, "New part")
        .click();
    step_until(&mut h, "its tab", |h| h.state().tabs().len() == 1);
    assert!(!anywhere(&h, "New document"));
    assert_eq!(h.state().tabs()[0].name(), "Untitled");
    step_until(&mut h, "the start page left", |h| !start_shown(h));
    home_button(&h).click();
    step_until(&mut h, "Home", start_shown);
    assert!(recent_names(&h).is_empty());
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
fn the_start_pages_open_asks_the_panel_as_file_open_does_and_its_pick_opens_and_is_recent() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let path = f.file("example.silico", EXAMPLE);
    let file = f.recents_file();
    let mut h = f.app_made(|app| {
        app.remembering(Recents::at(file))
            .browsing(Box::new(browser.clone()))
    });
    recents_read(&mut h);
    step(&mut h);
    start_page(&h)
        .get_by_role_and_label(Role::Button, "Open…")
        .click();
    open_panel_asked(&mut h, &browser, 1);
    browser.answer(Some(&path));
    step_until(&mut h, "its tab", |h| !h.state().tabs().is_empty());
    assert_eq!(h.state().tabs()[0].path.as_deref(), path.to_str());
    step_until(&mut h, "the start page left", |h| !start_shown(h));
    // Listed in Recent, as a typed path's open is.
    home_button(&h).click();
    step_until(&mut h, "listed", |h| {
        start_shown(h) && recent_names(h) == ["example.silico"]
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn command_n_and_command_o_make_a_part_and_ask_for_open_from_the_start_page() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::O);
    step_until(&mut h, "Open document", |h| anywhere(h, "Open document"));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "it gone", |h| !anywhere(h, "Open document"));
    assert!(h.state().tabs().is_empty());
    // Command+N: an untitled part at once, no question.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::N);
    step_until(&mut h, "its tab", |h| h.state().tabs().len() == 1);
    assert!(!anywhere(&h, "New document"));
    assert_eq!(h.state().tabs()[0].name(), "Untitled");
    assert_eq!(h.state().tabs()[0].path, None);
    step_until(&mut h, "the start page left", |h| !start_shown(h));
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
    assert_eq!(again.state().tabs()[0].path.as_deref(), Some(two));
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
        h.state().tabs()[0].path.as_deref() == copy.to_str()
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
        shows(h, "Timeline", "Pinx") && !h.state().tabs()[0].redoable()
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

// ---- The toolbar (docs/design/m5-ui/DESIGN.md, *Layout*; styles.css's `.tb-*` and
// `.btn-finish`; app.js's `renderToolbar` and `CMD`, interact.js's drop-downs) ----

/// The labels of `node`'s buttons, in the tree's order.
fn buttons_of(node: &egui_kittest::Node<'_>) -> Vec<String> {
    node.query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

/// The labels of `node`'s buttons that are on.
fn on_of(node: &egui_kittest::Node<'_>) -> Vec<String> {
    node.query_all_by_role(Role::Button)
        .filter(|node| !node.accesskit_node().is_disabled())
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

/// The toolbar's panels: every toolbar but the canvas's navigation bar, in order.
fn panels_of<'a>(nodes: impl Iterator<Item = egui_kittest::Node<'a>>) -> Vec<String> {
    nodes
        .filter_map(|node| node.accesskit_node().label())
        .filter(|label| label != "Navigation bar")
        .collect()
}

/// The toolbar's tabs, each with whether it's the one shown.
fn tabs_of<'a>(nodes: impl Iterator<Item = egui_kittest::Node<'a>>) -> Vec<(String, bool)> {
    nodes
        .map(|node| {
            let shown = toggled(&node) == Some(true);
            (node.accesskit_node().label().unwrap_or_default(), shown)
        })
        .collect()
}

/// The texts of the labels the window shows (a tooltip's lines among them).
fn label_texts<'a>(nodes: impl Iterator<Item = egui_kittest::Node<'a>>) -> Vec<String> {
    nodes
        .filter_map(|node| {
            let node = node.accesskit_node();
            node.label().or_else(|| node.value())
        })
        .collect()
}

/// CREATE's row, as app.js:394 lays it out: Create Sketch and the sweeps.
const CREATE_ROW: [&str; 5] = ["Create Sketch", "Pad", "Pocket", "Revolve", "Groove"];

#[test]
fn the_toolbar_holds_solids_panels_their_buttons_in_the_prototypes_order_and_sizes() {
    let f = fixture();
    let h = f.themed_with(&["example.silico"]);
    // `.tb-tabs`: SOLID, shown; no SKETCH with no sketch open.
    assert_eq!(
        tabs_of(h.query_all_by_role(Role::Tab)),
        [("SOLID".to_owned(), true)]
    );
    assert!(h.query_by_role_and_label(Role::Label, "DESIGN").is_some());
    // CREATE, MODIFY, INSPECT, SELECT (DESIGN.md, *Layout*), each its row then its label.
    assert_eq!(
        panels_of(h.query_all_by_role(Role::Toolbar)),
        ["CREATE", "MODIFY", "INSPECT", "SELECT"]
    );
    let panel = |label: &'static str| h.get_by_role_and_label(Role::Toolbar, label);
    let mut create: Vec<&str> = CREATE_ROW.to_vec();
    create.push("CREATE");
    assert_eq!(buttons_of(&panel("CREATE")), create);
    for primitive in PRIMITIVES {
        assert!(
            panel("CREATE").query_by_label(primitive).is_none(),
            "{primitive}"
        );
    }
    assert_eq!(
        buttons_of(&panel("MODIFY")),
        [
            "Fillet",
            "Chamfer",
            "Union",
            "Subtract",
            "Intersect",
            "MODIFY"
        ]
    );
    assert_eq!(buttons_of(&panel("INSPECT")), ["Measure", "INSPECT"]);
    assert_eq!(
        buttons_of(&panel("SELECT")),
        ["Faces", "Edges", "Vertices", "SELECT"]
    );
    // On: Create Sketch (sketch mode can open), every dialog (the example has a body, so
    // the cuts, the fillet, and the chamfer too), and the drop-downs; off: what isn't built.
    let mut on: Vec<&str> = CREATE_ROW.to_vec();
    on.push("CREATE");
    assert_eq!(on_of(&panel("CREATE")), on);
    assert_eq!(on_of(&panel("MODIFY")), ["Fillet", "Chamfer", "MODIFY"]);
    assert_eq!(on_of(&panel("INSPECT")), ["INSPECT"]);
    // SELECT's filters toggle the selection's filter (M5.7), each pressed while its kind is
    // taken: all three at first.
    assert_eq!(
        on_of(&panel("SELECT")),
        ["Faces", "Edges", "Vertices", "SELECT"]
    );
    for label in ["Faces", "Edges", "Vertices"] {
        assert_eq!(toggled(&panel("SELECT").get_by_label(label)), Some(true));
    }
    // Sizes (`--toolbar-tabs-h: 24px`, `--toolbar-h: 52px`, `.tb-btn` 30 square 1 apart
    // with `.tb-panel`'s 4 above), from the toolbar's top left: DESIGN's top (`.tb-ws`, the
    // tabs' row's height), and its left less `.tb-tabs`' 8. The buttons' tops 24 + 4 down;
    // the first's left after `.tb-panels`' 6 and the first panel's 6.
    let design = h.get_by_role_and_label(Role::Label, "DESIGN").rect();
    let (left, top) = (design.left() - 8.0, design.top());
    // Under the 30 high app bar, whose tabs run from its top.
    assert_eq!(top, bar_button(&h, "example.silico").rect().top() + 30.0);
    assert_eq!(design.height(), 24.0 - 1.0);
    let rect = |label: &str| panel("CREATE").get_by_label(label).rect();
    for label in CREATE_ROW {
        assert_eq!(
            rect(label).size(),
            eframe::egui::vec2(30.0, 30.0),
            "{label}"
        );
        assert_eq!(rect(label).top(), top + 24.0 + 4.0, "{label}");
    }
    assert_eq!(rect("Create Sketch").left(), left + 6.0 + 6.0);
    for pair in CREATE_ROW.windows(2) {
        assert_eq!(
            rect(pair[1]).left() - rect(pair[0]).right(),
            1.0,
            "{pair:?}"
        );
    }
    // `.tb-label`: 1 under the row, 14 of line and 1 of padding over and under; MODIFY's
    // first button after CREATE's last, CREATE's 6 of padding (`.tb-panel`'s `padding: 4px
    // 6px 0`), its 1 of `border-right`, and MODIFY's 6.
    let label = rect("CREATE");
    assert_eq!(label.top(), rect("Groove").bottom() + 1.0);
    assert_eq!(label.height(), 16.0);
    let fillet = panel("MODIFY").get_by_label("Fillet").rect();
    assert_eq!(fillet.left() - rect("Groove").right(), 6.0 + 1.0 + 6.0);
    // `.tb-tab`: 22 high, on the tabs' row's border; the toolbar ends 1 under its panels.
    let solid = h.get_by_role_and_label(Role::Tab, "SOLID").rect();
    assert_eq!(solid.height(), 22.0);
    assert_eq!(solid.bottom(), top + 24.0 - 1.0);
    assert_eq!(solid.left(), design.right() + 2.0 + 6.0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn each_toolbar_button_names_its_command_and_says_what_it_does_or_that_it_isnt_built() {
    let f = fixture();
    let mut h = f.themed_with(&["example.silico"]);
    // app.js's `CMD`: each command's `label` and `desc`; one not built says so after it.
    for (panel, button, lines) in [
        (
            "CREATE",
            "Pad",
            vec!["Extrudes a sketch's profile into solid"],
        ),
        (
            "CREATE",
            "Pocket",
            vec!["Cuts a sketch's profile out of the solid"],
        ),
        ("CREATE", "Revolve", vec!["Sweeps a profile around an axis"]),
        (
            "CREATE",
            "Groove",
            vec!["Cuts a profile swept around an axis"],
        ),
        (
            "CREATE",
            "Create Sketch",
            vec!["Sketches on an origin plane"],
        ),
        ("MODIFY", "Fillet", vec!["Rounds the selected edges"]),
        ("MODIFY", "Chamfer", vec!["Bevels the selected edges"]),
        ("MODIFY", "Union", vec!["Not built yet"]),
        (
            "INSPECT",
            "Measure",
            vec!["Distance, angle, and size of what you select. Not built yet"],
        ),
        (
            "SELECT",
            "Edges",
            vec!["Toggles whether edges can be selected"],
        ),
    ] {
        pointer_away(&mut h);
        h.get_by_role_and_label(Role::Toolbar, panel)
            .get_by_label(button)
            .hover();
        step_until(&mut h, "its tooltip", |h| {
            h.query_by_role_and_label(Role::Label, button).is_some()
        });
        step(&mut h);
        let shown = label_texts(h.query_all_by_role(Role::Label));
        let at = shown.iter().position(|text| text == button).unwrap();
        // The title, then the line, if any, and nothing more of the tooltip's.
        let after: Vec<&str> = shown[at + 1..].iter().map(String::as_str).collect();
        assert!(after.starts_with(&lines), "{button}: {shown:?}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_drop_downs_hold_the_prototypes_entries_and_creates_open_their_dialogs() {
    let f = fixture();
    let mut h = f.themed_with(&["example.silico"]);
    let open = |h: &mut AppHarness, label: &str| {
        pointer_away(h);
        h.get_by_role_and_label(Role::Toolbar, label)
            .get_by_role_and_label(Role::Button, label)
            .click();
        step(h);
        step(h);
    };
    // interact.js:172: the filters under a heading, all of them, each with its key, and
    // Clear selection (Esc), which the app has: off with nothing selected.
    open(&mut h, "SELECT");
    let menu = h.get_by_role_and_label(Role::Menu, "SELECT");
    let entries = buttons_of(&menu);
    assert_eq!(entries.len(), 5, "{entries:?}");
    for (entry, label) in entries.iter().zip(["Faces", "Edges", "Vertices"]) {
        assert!(entry.starts_with(label), "{entries:?}");
    }
    assert_eq!(entries[3], "Select all filters");
    assert!(entries[4].starts_with("Clear selection"), "{entries:?}");
    assert!(menu.query_by_label("SELECTION FILTERS").is_some());
    assert_eq!(on_of(&menu), entries[..4]);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    // interact.js:170: Create Sketch, the sweeps, and the primitives under a heading.
    open(&mut h, "CREATE");
    let menu = h.get_by_role_and_label(Role::Menu, "CREATE");
    let entries = buttons_of(&menu);
    assert_eq!(entries.len(), 10, "{entries:?}");
    assert!(entries[0].starts_with("Create Sketch"), "{entries:?}");
    assert_eq!(entries[1..5], CREATE_ROW[1..]);
    assert_eq!(entries[5..], PRIMITIVES);
    assert!(menu.query_by_label("PRIMITIVES").is_some());
    // Create Sketch too, now that sketch mode can open.
    let mut on: Vec<&str> = CREATE_ROW.iter().chain(&PRIMITIVES).copied().collect();
    on.sort_unstable();
    let mut menu_on = on_of(&menu);
    menu_on.sort_unstable();
    assert_eq!(menu_on, on);
    // An entry opens its dialog, as the row's button does, and the menu closes.
    menu.get_by_label("Cylinder").click();
    step_until(&mut h, "the dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Cylinder")
            .is_some()
    });
    // The dialog shows in the click's own frame, docked in Properties; egui closes a popup
    // at its frame's end, so the menu is gone the frame after.
    step(&mut h);
    assert!(h.query_by_role_and_label(Role::Menu, "CREATE").is_none());
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
    step_until(&mut h, "the dialog closed", |h| {
        h.query_by_role(Role::Dialog).is_none()
    });
    settled(&mut h);
    // interact.js:171: the edge tools, and the booleans under a heading, the edge tools
    // built.
    open(&mut h, "MODIFY");
    let menu = h.get_by_role_and_label(Role::Menu, "MODIFY");
    assert_eq!(
        buttons_of(&menu),
        ["Fillet", "Chamfer", "Union", "Subtract", "Intersect"]
    );
    assert!(menu.query_by_label("COMBINE").is_some());
    assert_eq!(on_of(&menu), ["Fillet", "Chamfer"]);
    // An entry not built says so, as its button does (Union has no line of its own).
    h.get_by_role_and_label(Role::Menu, "MODIFY")
        .get_by_label("Union")
        .hover();
    step_until(&mut h, "its tooltip", |h| {
        h.query_by_role_and_label(Role::Label, "Not built yet")
            .is_some()
    });
    open(&mut h, "INSPECT");
    let menu = h.get_by_role_and_label(Role::Menu, "INSPECT");
    assert_eq!(buttons_of(&menu), ["Measure"]);
    assert!(on_of(&menu).is_empty());
    // With a feature selected, Clear selection clears it, as Esc does.
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    settled(&mut h);
    select(&mut h, "Pin");
    open(&mut h, "SELECT");
    let menu = h.get_by_role_and_label(Role::Menu, "SELECT");
    let clear = menu
        .query_all_by_role(Role::Button)
        .find(|node| {
            node.accesskit_node()
                .label()
                .is_some_and(|label| label.starts_with("Clear selection"))
        })
        .unwrap();
    assert!(!clear.accesskit_node().is_disabled());
    clear.click();
    step(&mut h);
    step(&mut h);
    assert!(h.state().tabs()[0].selection().is_empty());
    assert!(h.query_by_role_and_label(Role::Menu, "SELECT").is_none());
    // With a feature selected and a dialog open, Clear selection is off: the dialog's
    // Cancel (Esc) comes first.
    select(&mut h, "Pin");
    create(&mut h, "Box");
    settled(&mut h);
    assert!(h.state().tabs()[0].selection().feature().is_some());
    open(&mut h, "SELECT");
    let clear = h
        .get_by_role_and_label(Role::Menu, "SELECT")
        .query_all_by_role(Role::Button)
        .find(|node| {
            node.accesskit_node()
                .label()
                .is_some_and(|label| label.starts_with("Clear selection"))
        })
        .unwrap();
    assert!(clear.accesskit_node().is_disabled());
    assert_eq!(never_waits::waits(), 0);
}

/// The toolbar's state as a test stubs it (sketch mode's among it), and what it was asked.
struct ToolbarStub {
    in_sketch: bool,
    create_sketch: Option<bool>,
    off: Vec<silico_app::dialog::Kind>,
    selected: bool,
    filter: silico_app::selection::Filter,
    acts: Vec<silico_app::toolbar::Act>,
}

impl ToolbarStub {
    fn new() -> Self {
        Self {
            in_sketch: false,
            create_sketch: None,
            off: Vec::new(),
            selected: false,
            filter: silico_app::selection::Filter::default(),
            acts: Vec::new(),
        }
    }
}

type ToolbarHarness = Harness<'static, ToolbarStub>;

/// The toolbar alone, drawn in the theme from `stub`, by `builder`.
fn toolbar_with(
    builder: egui_kittest::HarnessBuilder<ToolbarStub>,
    stub: ToolbarStub,
) -> ToolbarHarness {
    let mut h = builder
        .with_size((1280.0, 600.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, stub: &mut ToolbarStub| {
                let off = stub.off.clone();
                let can_open = move |kind| !off.contains(&kind);
                let shown = silico_app::toolbar::Shown {
                    in_sketch: stub.in_sketch,
                    create_sketch: stub.create_sketch,
                    can_open: &can_open,
                    selected: stub.selected,
                    filter: stub.filter,
                };
                // The whole window, as the app's frame has it, not the harness's panel inside
                // its margin.
                let window = eframe::egui::UiBuilder::new().max_rect(ui.ctx().content_rect());
                let act = ui
                    .scope_builder(window, |ui| silico_app::toolbar::show(ui, &shown))
                    .inner;
                stub.acts.extend(act);
            },
            stub,
        );
    silico_app::theme::apply(&h.ctx);
    h.step();
    h.step();
    h
}

fn toolbar(stub: ToolbarStub) -> ToolbarHarness {
    toolbar_with(Harness::builder(), stub)
}

/// Frames stepped until `done` holds, for a second of the harness's time at most, and one
/// more: egui lays a new popup out unseen in its first frame, and places it in its next.
#[track_caller]
fn toolbar_until(h: &mut ToolbarHarness, what: &str, done: impl Fn(&ToolbarHarness) -> bool) {
    for _ in 0..120 {
        h.step();
        if done(h) {
            h.step();
            return;
        }
    }
    panic!("never {what}");
}

/// The pointer away from the toolbar, a frame drawn.
fn toolbar_pointer_away(h: &mut ToolbarHarness) {
    h.hover_at(eframe::egui::pos2(640.0, 590.0));
    h.step();
    h.step();
}

#[test]
fn the_sketch_tab_and_a_blue_finish_sketch_show_while_a_sketch_is_open() {
    use silico_app::toolbar::Act;
    let mut h = toolbar(ToolbarStub::new());
    assert_eq!(
        tabs_of(h.query_all_by_role(Role::Tab)),
        [("SOLID".to_owned(), true)]
    );
    assert!(h.query_by_label("Finish Sketch").is_none());
    // A sketch open (#351's `tab.in_sketch()`): SKETCH after SOLID, and shown; its panels
    // (app.js:384), none of their tools built in the toolbar yet; Finish Sketch, on.
    h.state_mut().in_sketch = true;
    h.step();
    h.step();
    assert_eq!(
        tabs_of(h.query_all_by_role(Role::Tab)),
        [("SOLID".to_owned(), false), ("SKETCH".to_owned(), true)]
    );
    assert_eq!(
        panels_of(h.query_all_by_role(Role::Toolbar)),
        ["CREATE", "MODIFY", "CONSTRAINTS", "INSPECT"]
    );
    let panel = |h: &ToolbarHarness, label: &str| {
        buttons_of(&h.get_by_role_and_label(Role::Toolbar, label))
    };
    assert_eq!(
        panel(&h, "CREATE"),
        [
            "Line",
            "Rectangle",
            "Circle",
            "Arc",
            "Point",
            "Construction",
            "CREATE"
        ]
    );
    assert_eq!(panel(&h, "MODIFY"), ["Trim", "Extend", "Split", "MODIFY"]);
    assert_eq!(
        panel(&h, "CONSTRAINTS"),
        [
            "Smart Dimension",
            "Horizontal",
            "Vertical",
            "Coincident",
            "Tangent",
            "Equal",
            "Parallel",
            "Perpendicular",
            "Midpoint",
            "Symmetric",
            "Fix",
            "CONSTRAINTS"
        ]
    );
    assert_eq!(panel(&h, "INSPECT"), ["Measure", "INSPECT"]);
    for label in ["CREATE", "MODIFY", "CONSTRAINTS", "INSPECT"] {
        assert_eq!(
            on_of(&h.get_by_role_and_label(Role::Toolbar, label)),
            [label]
        );
    }
    // `.tb-finish`: at the panels' right end, inside `.tb-panels`' 6 and its own 8; 30
    // high, centred in the 52 of panels under the 24 of tabs.
    let finish = h.get_by_role_and_label(Role::Button, "Finish Sketch");
    assert!(!finish.accesskit_node().is_disabled());
    let rect = finish.rect();
    assert_eq!(rect.right(), 1280.0 - 6.0 - 8.0);
    assert_eq!(rect.height(), 30.0);
    assert_eq!(rect.center().y, 24.0 + 52.0 / 2.0);
    // Its tooltip: its name, its key (app.js: `Mod Enter`), and the prototype's line.
    finish.hover();
    toolbar_until(&mut h, "its tooltip", |h| {
        h.query_by_role_and_label(Role::Label, "Keeps your changes and rebuilds")
            .is_some()
    });
    h.step();
    let command = if h.ctx.os() == eframe::egui::os::OperatingSystem::Mac {
        "⌘"
    } else {
        "Ctrl"
    };
    let texts = |h: &ToolbarHarness| -> Vec<String> {
        h.output()
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                eframe::egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    };
    toolbar_until(&mut h, "its tooltip drawn", |h| {
        texts(h)
            .iter()
            .any(|text| text == "Keeps your changes and rebuilds")
    });
    let texts = texts(&h);
    assert!(texts.iter().any(|text| text == command), "{texts:?}");
    // Enter as the prototype's KEYNAME names it, as the registry's caps do.
    assert!(texts.iter().any(|text| text == "↵"), "{texts:?}");
    // A label's drop-down lists its row's commands, CONSTRAINTS' among them.
    let label = |h: &ToolbarHarness| {
        h.get_by_role_and_label(Role::Toolbar, "CONSTRAINTS")
            .get_by_role_and_label(Role::Button, "CONSTRAINTS")
            .click();
    };
    label(&h);
    toolbar_until(&mut h, "CONSTRAINTS' drop-down", |h| {
        h.query_by_role_and_label(Role::Menu, "CONSTRAINTS")
            .is_some()
    });
    let mut row = panel(&h, "CONSTRAINTS");
    row.pop();
    assert_eq!(
        buttons_of(&h.get_by_role_and_label(Role::Menu, "CONSTRAINTS")),
        row
    );
    label(&h);
    toolbar_until(&mut h, "CONSTRAINTS' drop-down closed", |h| {
        h.query_by_role_and_label(Role::Menu, "CONSTRAINTS")
            .is_none()
    });
    // A click says Finish Sketch, which the caller sends to sketch mode's finish.
    h.get_by_role_and_label(Role::Button, "Finish Sketch")
        .click();
    h.step();
    assert_eq!(h.state().acts, [Act::FinishSketch]);
    // The sketch closed: SOLID's again, and no Finish Sketch.
    h.state_mut().in_sketch = false;
    h.step();
    h.step();
    assert_eq!(
        tabs_of(h.query_all_by_role(Role::Tab)),
        [("SOLID".to_owned(), true)]
    );
    assert!(h.query_by_label("Finish Sketch").is_none());
    assert_eq!(h.state().acts, [Act::FinishSketch]);
}

#[test]
fn create_sketch_waits_for_sketch_mode_then_offers_the_origin_planes_from_its_button_and_creates_drop_down()
 {
    use silico_app::toolbar::Act;
    use silico_core::document::OriginPlane;
    let create_sketch = |h: &ToolbarHarness| {
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .get_by_label("Create Sketch")
            .accesskit_node()
            .is_disabled()
    };
    // No sketch mode: off, and its tooltip says it isn't built.
    let mut h = toolbar(ToolbarStub::new());
    assert!(create_sketch(&h));
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label("Create Sketch")
        .hover();
    toolbar_until(&mut h, "its tooltip", |h| {
        h.query_by_role_and_label(Role::Label, "Sketches on an origin plane. Not built yet")
            .is_some()
    });
    // Sketch mode, but it can't open now (#351's `tab.can_sketch()` false): off, built.
    let mut stub = ToolbarStub::new();
    stub.create_sketch = Some(false);
    let mut h = toolbar(stub);
    assert!(create_sketch(&h));
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label("Create Sketch")
        .hover();
    toolbar_until(&mut h, "its tooltip", |h| {
        h.query_by_role_and_label(Role::Label, "Sketches on an origin plane")
            .is_some()
    });
    assert!(h.query_by_label_contains("Not built").is_none());
    // It can: on, and a click opens the origin's planes; one clicked says it.
    let mut stub = ToolbarStub::new();
    stub.create_sketch = Some(true);
    let mut h = toolbar(stub);
    assert!(!create_sketch(&h));
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label("Create Sketch")
        .click();
    toolbar_until(&mut h, "its planes", |h| {
        h.query_by_role_and_label(Role::Menu, "Create Sketch")
            .is_some()
    });
    let planes = h.get_by_role_and_label(Role::Menu, "Create Sketch");
    assert_eq!(buttons_of(&planes), ["XY plane", "XZ plane", "YZ plane"]);
    planes.get_by_label("XZ plane").click();
    h.step();
    h.step();
    assert_eq!(h.state().acts, [Act::CreateSketch(OriginPlane::Xz)]);
    assert!(
        h.query_by_role_and_label(Role::Menu, "Create Sketch")
            .is_none()
    );
    // And from CREATE's drop-down, as a menu of its own.
    toolbar_pointer_away(&mut h);
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_role_and_label(Role::Button, "CREATE")
        .click();
    toolbar_until(&mut h, "CREATE's drop-down", |h| {
        h.query_by_role_and_label(Role::Menu, "CREATE").is_some()
    });
    h.get_by_role_and_label(Role::Menu, "CREATE")
        .query_all_by_role(Role::Button)
        .find(|node| {
            node.accesskit_node()
                .label()
                .is_some_and(|label| label.starts_with("Create Sketch"))
        })
        .unwrap()
        .click();
    toolbar_until(&mut h, "its planes", |h| {
        h.query_by_role_and_label(Role::Menu, "Create Sketch")
            .is_some()
    });
    h.get_by_role_and_label(Role::Menu, "Create Sketch")
        .get_by_label("YZ plane")
        .click();
    h.step();
    h.step();
    assert_eq!(
        h.state().acts,
        [
            Act::CreateSketch(OriginPlane::Xz),
            Act::CreateSketch(OriginPlane::Yz)
        ]
    );
    assert!(h.query_by_role_and_label(Role::Menu, "CREATE").is_none());
}

#[test]
fn a_dialog_that_cant_open_is_off_in_creates_row_and_drop_down_and_one_that_can_says_so_once() {
    use silico_app::dialog::Kind;
    use silico_app::toolbar::Act;
    let mut stub = ToolbarStub::new();
    stub.off = vec![Kind::Pocket, Kind::Groove];
    let mut h = toolbar(stub);
    let create = h.get_by_role_and_label(Role::Toolbar, "CREATE");
    let off = |label: &str| create.get_by_label(label).accesskit_node().is_disabled();
    assert!(off("Pocket") && off("Groove"));
    assert!(!off("Pad") && !off("Revolve"));
    create.get_by_label("Pad").click();
    h.step();
    assert_eq!(h.state().acts, [Act::Dialog(Kind::Pad)]);
    // A click on an off one says nothing.
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_label("Pocket")
        .click();
    h.step();
    assert_eq!(h.state().acts, [Act::Dialog(Kind::Pad)]);
    toolbar_pointer_away(&mut h);
    h.get_by_role_and_label(Role::Toolbar, "CREATE")
        .get_by_role_and_label(Role::Button, "CREATE")
        .click();
    toolbar_until(&mut h, "CREATE's drop-down", |h| {
        h.query_by_role_and_label(Role::Menu, "CREATE").is_some()
    });
    let menu = h.get_by_role_and_label(Role::Menu, "CREATE");
    let menu_off = |label: &str| menu.get_by_label(label).accesskit_node().is_disabled();
    assert!(menu_off("Pocket") && menu_off("Groove"));
    assert!(!menu_off("Pad") && !menu_off("Box") && !menu_off("Torus"));
    menu.get_by_label("Torus").click();
    h.step();
    h.step();
    assert_eq!(
        h.state().acts,
        [Act::Dialog(Kind::Pad), Act::Dialog(Kind::Torus)]
    );
}

#[test]
fn the_toolbar_draws_solid_in_gray_200_and_its_hovered_button_in_gray_300_on_lavapipe() {
    on_lavapipe(
        "the_toolbar_draws_solid_in_gray_200_and_its_hovered_button_in_gray_300_on_lavapipe",
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
            let pad = h
                .get_by_role_and_label(Role::Toolbar, "CREATE")
                .get_by_label("Pad")
                .rect();
            h.hover_at(pad.left_top() + eframe::egui::vec2(2.0, 2.0));
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let ppp = h.ctx.pixels_per_point();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            // SOLID, shown: Gray.200 (`.tb-tab.active`), in its left padding, clear of its
            // rounded corner and its text.
            let solid = h.get_by_role_and_label(Role::Tab, "SOLID").rect();
            let inside = eframe::egui::pos2(solid.left() + 5.0, solid.center().y);
            assert_eq!(pixel(inside), [0x21, 0x21, 0x21, 0xff], "{solid:?}");
            // The toolbar's own fill beside it, Gray.100 (`#toolbar`), and the tabs' border
            // under it, Gray.200 (`.tb-tabs`), and the toolbar's under the panels, Gray.250.
            let beside = eframe::egui::pos2(solid.right() + 40.0, solid.center().y);
            assert_eq!(pixel(beside), [0x0d, 0x0d, 0x0d, 0xff]);
            let under = eframe::egui::pos2(solid.right() + 40.0, solid.bottom() + 0.5);
            assert_eq!(pixel(under), [0x21, 0x21, 0x21, 0xff]);
            let bottom =
                eframe::egui::pos2(solid.right() + 40.0, solid.bottom() + 1.0 + 52.0 + 0.5);
            assert_eq!(pixel(bottom), [0x27, 0x26, 0x26, 0xff]);
            // CREATE's border at its right, Gray.200 (`.tb-panel`'s `border-right`), in the
            // column after its 6 of padding past Groove; the padding itself the toolbar's.
            let groove = h
                .get_by_role_and_label(Role::Toolbar, "CREATE")
                .get_by_label("Groove")
                .rect();
            let border = eframe::egui::pos2(groove.right() + 6.5, groove.center().y);
            assert_eq!(pixel(border), [0x21, 0x21, 0x21, 0xff], "{groove:?}");
            let padding = eframe::egui::pos2(groove.right() + 3.0, groove.center().y);
            assert_eq!(pixel(padding), [0x0d, 0x0d, 0x0d, 0xff], "{groove:?}");
            // Pad, hovered: Gray.300 (`.tb-btn:hover`), clear of its rounded corner and its
            // icon.
            let inside = eframe::egui::pos2(pad.left() + 4.0, pad.center().y);
            assert_eq!(pixel(inside), [0x2c, 0x2b, 0x2b, 0xff], "{pad:?}");
            assert_eq!(never_waits::waits(), 0);
        },
    );
}

#[test]
fn the_sketch_tab_and_finish_sketch_are_blue_500_on_lavapipe() {
    on_lavapipe(
        "the_sketch_tab_and_finish_sketch_are_blue_500_on_lavapipe",
        || {
            let state = lavapipe_render_state();
            let builder = Harness::builder().renderer(
                egui_kittest::wgpu::WgpuTestRenderer::from_render_state(state.clone()),
            );
            let mut stub = ToolbarStub::new();
            stub.in_sketch = true;
            let mut h = toolbar_with(builder, stub);
            toolbar_pointer_away(&mut h);
            let rendered = h.render().unwrap();
            let image = |x, y| rendered.get_pixel(x, y).0;
            let ppp = h.ctx.pixels_per_point();
            let pixel = |image: &dyn Fn(u32, u32) -> [u8; 4], at: eframe::egui::Pos2| {
                image((at.x * ppp) as u32, (at.y * ppp) as u32)
            };
            // SKETCH: Blue.500 (`.tb-tab.ctx`, `--blue`), in its left padding.
            let sketch = h.get_by_role_and_label(Role::Tab, "SKETCH").rect();
            let inside = eframe::egui::pos2(sketch.left() + 5.0, sketch.center().y);
            assert_eq!(
                pixel(&image, inside),
                [0x18, 0x6a, 0xdd, 0xff],
                "{sketch:?}"
            );
            // SOLID, not shown: no fill, the toolbar's Gray.100.
            let solid = h.get_by_role_and_label(Role::Tab, "SOLID").rect();
            let inside = eframe::egui::pos2(solid.left() + 5.0, solid.center().y);
            assert_eq!(pixel(&image, inside), [0x0d, 0x0d, 0x0d, 0xff], "{solid:?}");
            // Finish Sketch: Blue.500 (`.btn-finish`), in its left padding, clear of its
            // rounded corners and its icon.
            let finish = h
                .get_by_role_and_label(Role::Button, "Finish Sketch")
                .rect();
            let inside = eframe::egui::pos2(finish.left() + 6.0, finish.center().y);
            assert_eq!(
                pixel(&image, inside),
                [0x18, 0x6a, 0xdd, 0xff],
                "{finish:?}"
            );
            // Hovered: the prototype's `#2a78e8` (`.btn-finish:hover`); the pointer, which the
            // harness draws, at its right end, clear of where it's read.
            h.hover_at(eframe::egui::pos2(finish.right() - 8.0, finish.top() + 4.0));
            h.step();
            h.step();
            let rendered = h.render().unwrap();
            let image = |x, y| rendered.get_pixel(x, y).0;
            assert_eq!(
                pixel(&image, inside),
                [0x2a, 0x78, 0xe8, 0xff],
                "{finish:?}"
            );
        },
    );
}

// ---- The browser, as the UI prototype draws it (DESIGN.md, *Layout*; app.js's
// renderBrowser) ----

/// The browser's pane.
fn browser(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::Pane, "Browser")
}

/// The names of the browser's rows, in the tree's order: the document's, each header's, and
/// each row's (its eye's, caret's, and mark's words left out).
fn browser_rows(h: &AppHarness) -> Vec<String> {
    browser(h)
        .query_all(egui_kittest::kittest::by().recursive(true))
        .filter_map(|node| {
            let node = node.accesskit_node();
            match node.role() {
                Role::Label => node.value(),
                // A selectable row: a button with a toggled state (an eye or a caret has
                // none).
                Role::Button if node.toggled().is_some() => node.label(),
                _ => None,
            }
        })
        .filter(|name| !name.starts_with("error") && !name.starts_with("warning"))
        .collect()
}

#[test]
fn the_browser_lists_the_document_then_origin_bodies_and_sketches_each_row_with_its_eye() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    // app.js:427–455: the document's row (its file's name, `.silico` left off), Origin
    // (its point, its axes, its planes), Bodies, and Sketches, each a header with a caret;
    // the bodies and the sketch in the document's order.
    assert_eq!(
        browser_rows(&h),
        [
            "sketched", "Origin", "Origin", "X", "Y", "Z", "XY", "XZ", "YZ", "Bodies", "Washer",
            "Plate", "Sketches", "Triangle"
        ]
    );
    let bodies = browser(&h).get_by_role_and_label(Role::Group, "Bodies");
    assert!(bodies.query_by_label("Washer").is_some());
    assert!(bodies.query_by_label("Plate").is_some());
    assert!(bodies.query_by_label("Triangle").is_none());
    let sketches = browser(&h).get_by_role_and_label(Role::Group, "Sketches");
    assert!(sketches.query_by_label("Triangle").is_some());
    // An eye on each row of a thing that can be hidden; a caret on the document's and each
    // header's.
    for name in ["Origin", "X", "XY", "Washer", "Plate", "Triangle"] {
        assert!(
            browser(&h)
                .query_by_label(&format!("Hide {name}"))
                .is_some(),
            "{name}"
        );
    }
    for name in ["sketched", "Origin", "Bodies", "Sketches"] {
        assert!(
            browser(&h)
                .query_by_role_and_label(Role::Button, &format!("Collapse {name}"))
                .is_some(),
            "{name}"
        );
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_headers_caret_folds_its_rows_away_and_back_and_the_documents_folds_them_all() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    browser(&h).get_by_label("Collapse Bodies").click();
    step_until(&mut h, "Bodies folded", |h| {
        !shows(h, "Browser", "Washer") && shows(h, "Browser", "Expand Bodies")
    });
    assert_eq!(
        browser_rows(&h),
        [
            "sketched", "Origin", "Origin", "X", "Y", "Z", "XY", "XZ", "YZ", "Bodies", "Sketches",
            "Triangle"
        ]
    );
    assert!(
        browser(&h)
            .query_by_role_and_label(Role::Group, "Bodies")
            .is_none()
    );
    browser(&h).get_by_label("Expand Bodies").click();
    step_until(&mut h, "Bodies open", |h| shows(h, "Browser", "Washer"));
    // The document's caret: its row alone.
    browser(&h).get_by_label("Collapse sketched").click();
    step_until(&mut h, "all folded", |h| !shows(h, "Browser", "Triangle"));
    assert_eq!(browser_rows(&h), ["sketched"]);
    browser(&h).get_by_label("Expand sketched").click();
    step_until(&mut h, "all open", |h| shows(h, "Browser", "Triangle"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_empty_group_says_what_makes_its_first_row() {
    // The M2.1 example has a body and no sketch (app.js:446: `.tree-empty`).
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its body", |h| shows(h, "Browser", "Hole"));
    let sketches = browser(&h).get_by_role_and_label(Role::Group, "Sketches");
    assert!(
        sketches
            .query_by_label("None yet. Create Sketch starts one.")
            .is_some()
    );
    assert_eq!(never_waits::waits(), 0);
}

/// Two boxes of one name, each a body: the first 50 mm at the origin, the second 1 m, 2 m
/// along x.
const TWINS: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Cube", "position": [0, 0, 0], "length": 50, "width": 50, "height": 50},
    {"id": "a91cw0de", "type": "box", "v": 1, "name": "Cube", "position": [2000, 0, 0], "length": 1000, "width": 1000, "height": 1000}
  ]
}
"#;

#[test]
fn an_eye_hides_its_own_body_of_two_of_one_name_and_shows_closed() {
    let f = fixture();
    let (mut h, record) = f.drawn("twins.silico", TWINS);
    meshes_settled(&mut h);
    step_until(&mut h, "both eyes", |h| {
        browser(h).query_all_by_label("Hide Cube").count() == 2
    });
    // The second row's eye: that body alone hidden, its eye closed, the first's open.
    browser(&h)
        .query_all_by_label("Hide Cube")
        .nth(1)
        .unwrap()
        .click();
    step(&mut h);
    step(&mut h);
    assert_eq!(browser(&h).query_all_by_label("Hide Cube").count(), 1);
    assert_eq!(browser(&h).query_all_by_label("Show Cube").count(), 1);
    let eyes: Vec<String> = browser(&h)
        .query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .filter(|label| label.ends_with(" Cube"))
        .collect();
    assert_eq!(eyes, ["Hide Cube", "Show Cube"]);
    assert_eq!(
        h.state().tabs()[0].hidden,
        [silico_app::tab::Hideable::Feature(
            silico_core::document::FeatureId::new("a91cw0de").unwrap()
        )]
        .into_iter()
        .collect()
    );
    // The view draws the first body (the table's first) and not the second.
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn h_hides_and_shows_the_selected_body_but_not_while_a_field_is_typed_in() {
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    step_until(&mut h, "the browser's bodies", |h| {
        shows(h, "Browser", "Large")
    });
    browser(&h).get_by_label("Large").click();
    step_until(&mut h, "Large selected", |h| {
        toggled(&browser(h).get_by_label("Large")) == Some(true)
    });
    h.key_press(eframe::egui::Key::H);
    step(&mut h);
    step(&mut h);
    assert!(shows(&h, "Browser", "Show Large"));
    assert!(shows(&h, "Browser", "Hide Small"));
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1]);
    h.key_press(eframe::egui::Key::H);
    step(&mut h);
    step(&mut h);
    assert!(shows(&h, "Browser", "Hide Large"));
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1, 2]);
    // Typing in the Name field: the H is the field's, nothing hidden.
    name_field(&h).focus();
    step(&mut h);
    h.key_press(eframe::egui::Key::H);
    step(&mut h);
    step(&mut h);
    assert!(shows(&h, "Browser", "Hide Large"));
    assert!(h.state().tabs()[0].hidden.is_empty());
    assert_eq!(record.lock().unwrap().frames.last().unwrap().bodies, [1, 2]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn h_with_shift_or_alt_hides_nothing() {
    // H alone is Hide/Show (DESIGN.md, *Keyboard*); egui's own match would take Shift+H and
    // Alt+H for it too.
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    step_until(&mut h, "the browser's bodies", |h| {
        shows(h, "Browser", "Large")
    });
    browser(&h).get_by_label("Large").click();
    step_until(&mut h, "Large selected", |h| {
        toggled(&browser(h).get_by_label("Large")) == Some(true)
    });
    for modifiers in [eframe::egui::Modifiers::SHIFT, eframe::egui::Modifiers::ALT] {
        h.key_press_modifiers(modifiers, eframe::egui::Key::H);
        step(&mut h);
        step(&mut h);
        assert!(shows(&h, "Browser", "Hide Large"), "{modifiers:?}");
        assert!(h.state().tabs()[0].hidden.is_empty(), "{modifiers:?}");
        assert_eq!(
            record.lock().unwrap().frames.last().unwrap().bodies,
            [1, 2],
            "{modifiers:?}"
        );
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn h_hides_and_shows_the_selected_sketch() {
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    browser(&h).get_by_label("Triangle").click();
    step_until(&mut h, "Triangle selected", |h| {
        toggled(&browser(h).get_by_label("Triangle")) == Some(true)
    });
    h.key_press(eframe::egui::Key::H);
    step(&mut h);
    step(&mut h);
    assert!(shows(&h, "Browser", "Show Triangle"));
    assert!(shows(&h, "Browser", "Hide Plate"));
    h.key_press(eframe::egui::Key::H);
    step(&mut h);
    step(&mut h);
    assert!(shows(&h, "Browser", "Hide Triangle"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_hidden_body_stays_hidden_through_a_rename_and_its_undo_and_redo() {
    // Hidden by its feature, not its name: each rename, undo, and redo rebuilds and reads
    // the timeline again, and the body stays hidden and undrawn through each.
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    step_until(&mut h, "the browser's bodies", |h| {
        shows(h, "Browser", "Hide Large")
    });
    browser(&h).get_by_label("Hide Large").click();
    step(&mut h);
    browser(&h).get_by_label("Large").click();
    step_until(&mut h, "its properties", |h| {
        h.get_by_role_and_label(Role::Pane, "Properties")
            .query_by_role(Role::TextInput)
            .and_then(|field| field.value())
            .is_some_and(|value| value == "Large")
    });
    let drawn = |record: &Record| record.lock().unwrap().frames.last().unwrap().bodies.clone();
    // Renamed through the Name field: the cursor at its end, so "x" after the name.
    type_name(&mut h, "x");
    step_until(&mut h, "the renamed row", |h| {
        shows(h, "Browser", "Largex") && h.state().tabs()[0].edits.idle()
    });
    meshes_settled(&mut h);
    assert!(shows(&h, "Browser", "Show Largex"));
    assert!(!shows(&h, "Browser", "Hide Largex"));
    assert_eq!(drawn(&record), [1]);
    // Undone: the old name back, still hidden.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step_until(&mut h, "the rename undone", |h| {
        shows(h, "Browser", "Large") && !shows(h, "Browser", "Largex")
    });
    meshes_settled(&mut h);
    assert!(shows(&h, "Browser", "Show Large"));
    assert_eq!(drawn(&record), [1]);
    // Redone: the new name again, still hidden.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Y);
    step_until(&mut h, "the rename redone", |h| {
        shows(h, "Browser", "Largex")
    });
    meshes_settled(&mut h);
    assert!(shows(&h, "Browser", "Show Largex"));
    assert_eq!(drawn(&record), [1]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn h_with_a_feature_the_browser_doesnt_list_selected_hides_nothing() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // The pin, consumed by the cut: no row of its own.
    select(&mut h, "Pin");
    h.key_press(eframe::egui::Key::H);
    step(&mut h);
    step(&mut h);
    assert!(h.state().tabs()[0].hidden.is_empty());
    assert!(shows(&h, "Browser", "Hide Hole"));
    assert_eq!(never_waits::waits(), 0);
}

/// The SKETCHED part, its washer cut from its plate by a cut the kernel refuses: two
/// bodies, a sketch, and a feature in error.
const SKETCHED_FAILING: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000002", "type": "box", "v": 1, "name": "Washer", "position": [50, 0, 0], "length": 5, "width": 5, "height": 1},
    {"id": "b0000001", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, -10], "length": 40, "width": 40, "height": 10},
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 20, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []},
    {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Slot", "base": "b0000001", "tool": "b0000002"}
  ]
}
"#;

#[test]
fn the_browsers_rows_are_filled_as_the_prototype_fills_them_and_its_mark_is_red_on_lavapipe() {
    on_lavapipe(
        "the_browsers_rows_are_filled_as_the_prototype_fills_them_and_its_mark_is_red_on_lavapipe",
        || {
            let f = fixture();
            meshes_a_real_box(&f);
            f.kernel.refuse(|op| {
                matches!(op, Op::Boolean { .. })
                    .then(|| Error::new(ErrorCode::KernelFailed, "the slot came apart"))
            });
            let state = lavapipe_render_state();
            let present = silico_app::renderer::on_eframe(&state).expect("no renderer");
            let app = App::new(Arc::clone(&f.sessions)).presenting(Box::new(present));
            let mut h = {
                let _marked = never_waits::marked();
                Harness::builder()
                    .with_size((1280.0, 800.0))
                    .with_pixels_per_point(2.0)
                    .with_step_dt(1.0 / 60.0)
                    .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                        state.clone(),
                    ))
                    .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
            };
            silico_app::theme::apply(&h.ctx);
            h.set_render_every_step(true);
            let path = f.file("sketched.silico", SKETCHED_FAILING);
            h.state_mut().open(path.to_str().unwrap(), false);
            meshes_settled(&mut h);
            step_until(&mut h, "the rows and the mark", |h| {
                shows(h, "Browser", "Triangle") && shows(h, "Browser", "error")
            });
            // Plate selected, Washer hidden, the pointer over Triangle's row.
            browser(&h).get_by_label("Plate").click();
            step(&mut h);
            browser(&h).get_by_label("Hide Washer").click();
            step(&mut h);
            let triangle = browser(&h).get_by_label("Triangle").rect();
            h.hover_at(eframe::egui::pos2(triangle.center().x, triangle.center().y));
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let ppp = h.ctx.pixels_per_point();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            // In each row's padding at its left (`.row`'s `padding-left: 6px`), clear of a
            // focus ring's 1 point: Plate, selected, Blue.500 (`.row.selected`, `--blue`);
            // Triangle, hovered, Gray.250 (`.row:hover`, `--g250`); Washer, neither, the
            // panel's Gray.100 (`.panel`, `--g100`).
            let at_left = |name: &str| {
                let row = browser(&h).get_by_label(name).rect();
                eframe::egui::pos2(row.left() + 3.0, row.center().y)
            };
            assert_eq!(pixel(at_left("Plate")), [0x18, 0x6a, 0xdd, 0xff]);
            assert_eq!(pixel(at_left("Triangle")), [0x27, 0x26, 0x26, 0xff]);
            assert_eq!(pixel(at_left("Washer")), [0x0d, 0x0d, 0x0d, 0xff]);
            // The document's mark (`.r-err`): its icon's stroke in Red.550, `#ff3865`
            // (`--red`), within a step of rounding where the stroke covers a whole pixel.
            let mark = browser(&h).get_by_label("error").rect();
            let red = [0xff, 0x38, 0x65];
            let near = |rgba: [u8; 4]| (0..3).all(|c| rgba[c].abs_diff(red[c]) <= 3);
            let (x0, y0) = ((mark.left() * ppp) as u32, (mark.top() * ppp) as u32);
            let (x1, y1) = ((mark.right() * ppp) as u32, (mark.bottom() * ppp) as u32);
            let found = (y0..y1).any(|y| (x0..x1).any(|x| near(image.get_pixel(x, y).0)));
            assert!(found, "no {red:x?} in the mark at {mark:?}");
        },
    );
}

// ---- The timeline (DESIGN.md, *The idea in one line*; styles.css's `.tl-*`) ----

/// A plate, a triangle sketched on XY, a pad of the triangle on the plate, and a pocket of
/// it in the pad: one chain, each feature built from the one before it (the pad from the
/// plate and the sketch, the pocket from the pad and the sketch).
const LADDER: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000001", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, -10], "length": 40, "width": 40, "height": 10},
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 20, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []},
    {"id": "r0000002", "type": "pad", "v": 1, "name": "Raised", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 3, "reversed": false}},
    {"id": "q0000003", "type": "pocket", "v": 1, "name": "Cut", "base": "r0000002", "sketch": "s0000001", "extent": {"type": "one_side", "length": 1, "reversed": false}}
  ]
}
"#;

/// The timeline pane.
fn timeline(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::Pane, "Timeline")
}

/// What the timeline's item `name` says of its feature: its type and its state.
fn item_state(h: &AppHarness, name: &str) -> Option<String> {
    timeline(h)
        .query_by_label(name)
        .and_then(|node| node.accesskit_node().description())
}

/// The timeline's items' names, in order: the track's, not the summary's errors.
fn items(h: &AppHarness) -> Vec<String> {
    timeline(h)
        .get_by_role_and_label(Role::Group, silico_app::timeline::TRACK_LABEL)
        .query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

/// The kernel refuses the pocket's cut.
fn refuses_the_cut(f: &Fixture) {
    f.kernel.refuse(|op| {
        matches!(
            op,
            Op::Boolean {
                op: silico_protocol::BooleanOp::Cut,
                ..
            }
        )
        .then(|| Error::new(ErrorCode::KernelFailed, "the cut came apart"))
    });
}

/// The plate's height set to 12 through the tab, as the property panel sets a field: every
/// step after the plate's runs again.
fn plate_raised(h: &mut AppHarness, f: &Fixture) {
    let value = json::read(b"12").unwrap();
    let edit = silico_app::tab::Edit {
        feature: silico_core::document::FeatureId::new("b0000001").unwrap(),
        field: "height".to_owned(),
        value,
    };
    let _marked = never_waits::marked();
    h.state_mut().tabs_mut()[0].edit(&f.sessions, edit);
}

/// LADDER opened, its pocket refused, and its plate raised with the pad's fuse held: the
/// rebuild runs the pad's step, and holds it.
fn ladder_rebuilding(f: &Fixture, h: &mut AppHarness) -> Hold {
    step_until(h, "the pocket's failure", |h| {
        item_state(h, "Cut").as_deref() == Some("Pocket · Error")
    });
    let hold = f.kernel.hold(|op| {
        matches!(
            op,
            Op::Boolean {
                op: silico_protocol::BooleanOp::Fuse,
                ..
            }
        )
    });
    plate_raised(h, f);
    step_until(h, "the pad's step running", |h| {
        item_state(h, "Raised").as_deref() == Some("Pad · Rebuilding")
    });
    hold
}

#[test]
fn the_timeline_lists_each_feature_in_order_and_shows_the_rebuild_running_along_it() {
    let f = fixture();
    refuses_the_cut(&f);
    let (mut h, _) = f.opened("ladder.silico", LADDER);
    let hold = ladder_rebuilding(&f, &mut h);
    assert_eq!(items(&h), ["Plate", "Triangle", "Raised", "Cut"]);
    // The pad's step runs: the plate and the triangle are its inputs, so built before it
    // started; the pocket, built from the pad, waits. No mark while it runs.
    assert_eq!(item_state(&h, "Plate").as_deref(), Some("Box · Built"));
    assert_eq!(
        item_state(&h, "Triangle").as_deref(),
        Some("Sketch · Built")
    );
    assert_eq!(item_state(&h, "Cut").as_deref(), Some("Pocket · Queued"));
    assert!(timeline(&h).query_by_label("error").is_none());
    // A frame at once, for the spinner, while a step runs.
    step(&mut h);
    let delay = h
        .output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .map(|viewport| viewport.repaint_delay);
    assert_eq!(delay, Some(Duration::ZERO));
    // Released: the rebuild ends with the pocket failed again, red and marked, and the rest
    // as their health is (the plate's and the pad's fingerprints aren't stored: warned).
    hold.release();
    step_until(&mut h, "the rebuild's end", |h| {
        item_state(h, "Raised").is_some_and(|state| state != "Pad · Rebuilding")
            && item_state(h, "Cut").as_deref() == Some("Pocket · Error")
            && h.state().tabs()[0].timeline.as_ref().unwrap().revision
                == h.state().tabs()[0]
                    .status
                    .as_ref()
                    .unwrap()
                    .rebuild
                    .revision
    });
    for name in ["Plate", "Triangle", "Raised"] {
        let state = item_state(&h, name).unwrap();
        assert!(
            state.ends_with("Healthy") || state.ends_with("Warning"),
            "{name}: {state}"
        );
    }
    assert_eq!(
        showing(&h, "Timeline", "error").as_deref(),
        Some("error"),
        "the pocket's mark"
    );
    assert!(shows(&h, "Timeline", "1 error · 4 features"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_timelines_summary_counts_the_features_and_says_errors_only_when_there_are_some() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    assert!(shows(&h, "Timeline", "3 features"));
    assert!(timeline(&h).query_by_label_contains("error").is_none());
    let f = fixture();
    f.kernel.refuse(|op| {
        matches!(op, Op::Boolean { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "the cut came apart"))
    });
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "the cut's failure", |h| {
        shows(h, "Timeline", "1 error · 3 features")
    });
    // Empty: its words, and no feature counted wrongly.
    let empty = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": []}"#;
    let (mut h, _) = f.opened("empty.silico", empty);
    step_until(&mut h, "its words", |h| {
        shows(
            h,
            "Timeline",
            "Features appear here, in order, as you add them.",
        )
    });
    assert!(shows(&h, "Timeline", "0 features"));
    assert_eq!(never_waits::waits(), 0);
}

/// The timeline's item `name` dragged to `to`, a frame at each step of the way, the left
/// button held from its centre.
fn drag_item(h: &mut AppHarness, name: &str, to: eframe::egui::Pos2) {
    use eframe::egui::{Event, Modifiers, PointerButton};
    let from = timeline(h).get_by_label(name).rect().center();
    h.input_mut().events.push(Event::PointerMoved(from));
    step(h);
    h.input_mut().events.push(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
    });
    step(h);
    for t in [0.25, 0.5, 0.75, 1.0] {
        h.input_mut()
            .events
            .push(Event::PointerMoved(from + (to - from) * t));
        step(h);
    }
    h.input_mut().events.push(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
    });
    step(h);
}

#[test]
fn a_feature_dragged_along_the_timeline_moves_there_and_a_refused_move_is_said() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // The pin, dropped left of the block's centre: before it, as `move_feature` puts it.
    let block = timeline(&h).get_by_label("Block").rect();
    drag_item(
        &mut h,
        "Pin",
        block.left_center() + eframe::egui::vec2(3.0, 0.0),
    );
    step_until(&mut h, "the pin moved", |h| {
        items(h) == ["Pin", "Block", "Hole"]
    });
    let id = |id: &str| silico_core::document::FeatureId::new(id).unwrap();
    assert!(
        h.state().tabs()[0]
            .sent
            .contains(&Sent::Move(id("a91cw0de"), Some(id("k7f2qx3m"))))
    );
    // In the session's transaction, as an edit is: the file is as it was.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert!(anywhere(&h, "example.silico *"));
    // The hole before the pin, its tool: what the core refuses, said in the status bar,
    // the timeline as it was. The refusal's words are the core's own, from a session of
    // its own.
    let copy = f
        .sessions
        .open(f.file("copy.silico", EXAMPLE), false)
        .wait()
        .session
        .unwrap();
    let refused = f
        .sessions
        .move_feature(
            &copy.id,
            copy.revision,
            id("m3tq8v0z"),
            Some(id("a91cw0de")),
        )
        .wait()
        .result
        .unwrap_err();
    let said = silico_app::tab::said(&refused);
    let pin = timeline(&h).get_by_label("Pin").rect();
    drag_item(
        &mut h,
        "Hole",
        pin.left_center() + eframe::egui::vec2(3.0, 0.0),
    );
    step_until(&mut h, "the refusal said", |h| anywhere(h, &said));
    assert_eq!(items(&h), ["Pin", "Block", "Hole"]);
    // A drop where it is sends nothing.
    let sent = h.state().tabs()[0].sent.len();
    let block = timeline(&h).get_by_label("Block").rect();
    drag_item(
        &mut h,
        "Block",
        block.center() + eframe::egui::vec2(4.0, 0.0),
    );
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_move_asked_while_a_dialog_is_open_is_refused_and_said() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    double_click(&mut h, "Timeline", "Pin");
    step_until(&mut h, "its dialog", dialog_open);
    let sent = h.state().tabs()[0].sent.len();
    let block = timeline(&h).get_by_label("Block").rect();
    drag_item(
        &mut h,
        "Hole",
        block.left_center() + eframe::egui::vec2(3.0, 0.0),
    );
    step(&mut h);
    // Said in the status bar (its label, and its text, each a node).
    assert!(
        h.query_all_by_label("Close the dialog before moving a feature")
            .next()
            .is_some()
    );
    assert_eq!(h.state().tabs()[0].sent.len(), sent);
    assert_eq!(items(&h), ["Block", "Pin", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn hovering_a_timeline_item_highlights_the_faces_its_feature_made() {
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Large"));
    let number = |feature: &str| {
        view(&h)
            .bodies()
            .iter()
            .find(|(_, body)| body.feature.as_str() == feature)
            .map(|(number, _)| *number)
            .unwrap()
    };
    let large = number("a91cw0de");
    let last = |record: &Record| {
        record
            .lock()
            .unwrap()
            .frames
            .last()
            .unwrap()
            .highlights
            .clone()
    };
    assert!(last(&record).is_empty());
    // Hovered: each of the large box's six faces (a box makes six, every one its own), and
    // none of the small box's.
    let item = timeline(&h).get_by_label("Large").rect().center();
    h.hover_at(item);
    step(&mut h);
    step(&mut h);
    let faces: Vec<_> = (1..=6)
        .map(|n| (large, silico_app::renderer::Element::Face(n)))
        .collect();
    assert_eq!(last(&record), faces);
    // The pointer off it: nothing highlighted.
    h.hover_at(item + eframe::egui::vec2(0.0, -200.0));
    step(&mut h);
    step(&mut h);
    assert!(last(&record).is_empty());
    // A hidden body's faces aren't highlighted: it isn't drawn.
    browser(&h).get_by_label("Hide Large").click();
    step(&mut h);
    h.hover_at(item);
    step(&mut h);
    step(&mut h);
    assert!(last(&record).is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_timeline_draws_a_selected_item_blue_and_a_failed_one_red_on_lavapipe() {
    on_lavapipe(
        "the_timeline_draws_a_selected_item_blue_and_a_failed_one_red_on_lavapipe",
        || {
            // The fake's own meshes, a triangle a face: the bodies here aren't boxes.
            let f = fixture();
            refuses_the_cut(&f);
            let state = lavapipe_render_state();
            let present = silico_app::renderer::on_eframe(&state).expect("no renderer");
            let app = App::new(Arc::clone(&f.sessions)).presenting(Box::new(present));
            let mut h = {
                let _marked = never_waits::marked();
                Harness::builder()
                    .with_size((1280.0, 800.0))
                    .with_pixels_per_point(2.0)
                    .with_step_dt(1.0 / 60.0)
                    .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                        state.clone(),
                    ))
                    .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
            };
            silico_app::theme::apply(&h.ctx);
            h.set_render_every_step(true);
            let path = f.file("ladder.silico", LADDER);
            h.state_mut().open(path.to_str().unwrap(), false);
            meshes_settled(&mut h);
            step_until(&mut h, "the pocket's failure", |h| {
                item_state(h, "Cut").as_deref() == Some("Pocket · Error")
            });
            select(&mut h, "Plate");
            // The pointer away from the timeline, so no item is hovered.
            h.hover_at(eframe::egui::pos2(640.0, 300.0));
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let ppp = h.ctx.pixels_per_point();
            let pixel = |at: eframe::egui::Pos2| {
                image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
            };
            // The selected item (`.tl-item.selected`): Blue.500, `#186add`, inside its
            // left edge, clear of a focus ring's point, and of its icon.
            let plate = timeline(&h).get_by_label("Plate").rect();
            assert_eq!(plate.size(), eframe::egui::vec2(30.0, 30.0));
            let inside = eframe::egui::pos2(plate.left() + 2.5, plate.center().y);
            assert_eq!(pixel(inside), [0x18, 0x6a, 0xdd, 0xff], "{plate:?}");
            // The failed item (`.tl-item.err`): its bottom line Red.550, `#ff3865`
            // (`inset 0 -2px 0 var(--red)`), a point above its bottom; its mark
            // (`.badge.err`) `--red-fill`, `#ab0116`, at its centre, 1 in from the item's
            // top right; and the bar's Gray.150 (`#timeline`) beside the item.
            let cut = timeline(&h).get_by_label("Cut").rect();
            let line = eframe::egui::pos2(cut.center().x, cut.bottom() - 1.0);
            assert_eq!(pixel(line), [0xff, 0x38, 0x65, 0xff], "{cut:?}");
            let badge = eframe::egui::pos2(cut.right() - 1.0 - 4.5, cut.top() + 1.0 + 4.5);
            assert_eq!(pixel(badge), [0xab, 0x01, 0x16, 0xff], "{cut:?}");
            let bar = eframe::egui::pos2(cut.right() + 30.0, cut.center().y);
            assert_eq!(pixel(bar), [0x17, 0x17, 0x17, 0xff]);
            // Its icon is Red.550 too: within a step of rounding where a stroke covers a
            // whole pixel.
            let red = [0xff, 0x38, 0x65];
            let near = |rgba: [u8; 4]| (0..3).all(|c| rgba[c].abs_diff(red[c]) <= 3);
            let icon =
                eframe::egui::Rect::from_center_size(cut.center(), eframe::egui::vec2(20.0, 20.0));
            let (x0, y0) = ((icon.left() * ppp) as u32, (icon.top() * ppp) as u32);
            let (x1, y1) = ((icon.right() * ppp) as u32, (icon.bottom() * ppp) as u32);
            let found = (y0..y1).any(|y| (x0..x1).any(|x| near(image.get_pixel(x, y).0)));
            assert!(found, "no {red:x?} in the icon at {icon:?}");
            // A rebuild running: the pad's step spins, a ring in Gray.1000 (`.building`'s
            // `border-top-color: var(--g1000)`), which its icon (Gray.750) never is.
            let hold = ladder_rebuilding(&f, &mut h);
            for _ in 0..5 {
                step(&mut h);
            }
            let image = h.render().unwrap();
            let raised = timeline(&h).get_by_label("Raised").rect();
            let (x0, y0) = ((raised.left() * ppp) as u32, (raised.top() * ppp) as u32);
            let (x1, y1) = (
                (raised.right() * ppp) as u32,
                (raised.bottom() * ppp) as u32,
            );
            let white = (y0..y1).any(|y| (x0..x1).any(|x| image.get_pixel(x, y).0 == [0xff; 4]));
            assert!(white, "no spinner in {raised:?}");
            hold.release();
        },
    );
}

/// LADDER open, every feature settled.
fn ladder_opened(f: &Fixture) -> AppHarness {
    let (mut h, _) = f.opened("ladder.silico", LADDER);
    step_until(&mut h, "its features settled", |h| {
        items(h) == ["Plate", "Triangle", "Raised", "Cut"]
            && item_state(h, "Cut").is_some_and(|state| {
                state.starts_with("Pocket · ") && !state.ends_with("Not built")
            })
    });
    h
}

#[test]
fn after_a_move_nothing_is_shown_built_that_the_rebuild_running_may_not_have_built() {
    // The pocket dragged before the pad: `move_feature` rewires it onto the plate, and the
    // pad onto it. Its cut held, the pocket's step runs; the pad, after it now, isn't built.
    let f = fixture();
    let mut h = ladder_opened(&f);
    let listed = h.state().tabs()[0].timeline.clone().unwrap();
    let hold = f.kernel.hold(|op| {
        matches!(
            op,
            Op::Boolean {
                op: silico_protocol::BooleanOp::Cut,
                ..
            }
        )
    });
    let raised = timeline(&h).get_by_label("Raised").rect();
    drag_item(
        &mut h,
        "Cut",
        raised.left_center() + eframe::egui::vec2(3.0, 0.0),
    );
    // The new order shows while the rebuild is held: the timeline is read of the revision
    // the move made, beside its rebuild.
    step_until(&mut h, "the pocket's step running in the new order", |h| {
        items(h) == ["Plate", "Triangle", "Cut", "Raised"]
            && item_state(h, "Cut").as_deref() == Some("Pocket · Rebuilding")
    });
    // Its inputs in the new order, the plate and the triangle, are built; the pad is not.
    assert_eq!(item_state(&h, "Plate").as_deref(), Some("Box · Built"));
    assert_eq!(
        item_state(&h, "Triangle").as_deref(),
        Some("Sketch · Built")
    );
    assert_eq!(item_state(&h, "Raised").as_deref(), Some("Pad · Queued"));
    // The frames between the move's answer and its timeline's read: the tab still lists the
    // revision before the move, where the pocket's inputs were the pad and the triangle.
    // The rebuild is another revision's, so nothing is shown built, the pad least of all.
    let rebuilding = h.state().tabs()[0]
        .status
        .as_ref()
        .unwrap()
        .rebuild
        .revision;
    assert_ne!(listed.revision, rebuilding);
    h.state_mut().tabs_mut()[0].timeline = Some(listed);
    step(&mut h);
    assert_eq!(items(&h), ["Plate", "Triangle", "Raised", "Cut"]);
    assert_eq!(
        item_state(&h, "Cut").as_deref(),
        Some("Pocket · Rebuilding")
    );
    for name in ["Plate", "Triangle", "Raised"] {
        let state = item_state(&h, name).unwrap();
        assert!(state.ends_with("Queued"), "{name}: {state}");
    }
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn hovering_a_timeline_item_highlights_the_faces_named_as_its_features_not_its_bodys() {
    // One body, the pocket's, made by the chain: hovering the plate highlights the faces
    // named from the plate's (`b0000001.…`), the pad's those named from the pad's
    // (`r0000002.…`), each by its name's text, never the body's other faces.
    let f = fixture();
    let (mut h, record) = f.drawn("ladder.silico", LADDER);
    meshes_settled(&mut h);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Cut"));
    let (number, names): (u32, Vec<String>) = {
        let bodies = view(&h).bodies();
        assert_eq!(bodies.len(), 1, "one body");
        let (number, body) = bodies.iter().next().unwrap();
        assert_eq!(body.feature.as_str(), "q0000003");
        (
            *number,
            body.faces
                .iter()
                .map(|face| face.name.to_string())
                .collect(),
        )
    };
    let named = |prefix: &str| -> Vec<(u32, silico_app::renderer::Element)> {
        names
            .iter()
            .zip(1u32..)
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(_, n)| (number, silico_app::renderer::Element::Face(n)))
            .collect()
    };
    let plates = named("b0000001.");
    let pads = named("r0000002.");
    assert!(!plates.is_empty() && !pads.is_empty(), "{names:?}");
    assert!(plates.len() + pads.len() < names.len(), "{names:?}");
    let last = |record: &Record| {
        record
            .lock()
            .unwrap()
            .frames
            .last()
            .unwrap()
            .highlights
            .clone()
    };
    for (name, want) in [("Plate", &plates), ("Raised", &pads)] {
        h.hover_at(timeline(&h).get_by_label(name).rect().center());
        step(&mut h);
        step(&mut h);
        assert_eq!(&last(&record), want, "{name}: {names:?}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_move_is_undone_by_ctrl_z_and_the_file_is_never_touched() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let block = timeline(&h).get_by_label("Block").rect();
    drag_item(
        &mut h,
        "Pin",
        block.left_center() + eframe::egui::vec2(3.0, 0.0),
    );
    step_until(&mut h, "the pin moved", |h| {
        items(h) == ["Pin", "Block", "Hole"] && anywhere(h, "example.silico *")
    });
    step_until(&mut h, "Undo offered", |h| offered(h, "Undo"));
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::Z);
    step_until(&mut h, "the move undone", |h| {
        items(h) == ["Block", "Pin", "Hole"] && !anywhere(h, "example.silico *")
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_close_while_a_move_waits_before_its_transaction_opens_asks_first() {
    // The session's queue held: the move, and the `begin` sent for it, wait behind it, so
    // no transaction is open yet and the document hasn't changed. The move is still the
    // person's edit: Close asks, as it does for an edit.
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let (hold, check) = hold_queue(&f, &h, 0);
    let block = timeline(&h).get_by_label("Block").rect();
    drag_item(
        &mut h,
        "Pin",
        block.left_center() + eframe::egui::vec2(3.0, 0.0),
    );
    step(&mut h);
    assert!(h.state().tabs()[0].unsaved());
    h.get_by_label("Close example.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to example.silico before closing?")
    });
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the move shown", |h| {
        items(h) == ["Pin", "Block", "Hole"]
    });
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    assert_eq!(never_waits::waits(), 0);
}

// ---- The property panel's four jobs, and the status bar ----

/// The value on the property panel's row whose key is `key` (`.prop`): the label at its
/// right, on its line.
fn property(h: &AppHarness, key: &str) -> Option<String> {
    let pane = h.get_by_role_and_label(Role::Pane, "Properties");
    let key = pane
        .query_all_by_role(Role::Label)
        .find(|node| node.accesskit_node().value().as_deref() == Some(key))?
        .rect();
    pane.query_all_by_role(Role::Label).find_map(|node| {
        let rect = node.rect();
        ((rect.center().y - key.center().y).abs() < 1.0 && rect.left() > key.right())
            .then(|| node.accesskit_node().value())
            .flatten()
    })
}

/// The status bar's words that start with `start`, if it shows any (it's no pane: its
/// labels are the window's).
fn status_said(h: &AppHarness, start: &str) -> Option<String> {
    h.query_all_by_label_contains(start).find_map(|node| {
        let node = node.accesskit_node();
        let said = node.value().or_else(|| node.label())?;
        said.starts_with(start).then_some(said)
    })
}

#[test]
fn with_nothing_selected_the_property_panel_shows_the_documents_properties() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    step_until(&mut h, "its file compared", |h| {
        property(h, "File on disk").as_deref() == Some("Unchanged")
    });
    // The document's: opened at revision 1, its header's units, its three features, saved,
    // and its one body (the cut consumes the block and the pin).
    assert_eq!(property(&h, "Revision").as_deref(), Some("1"));
    assert_eq!(
        property(&h, "Units").as_deref(),
        Some("mm · 1 nm resolution")
    );
    assert_eq!(property(&h, "Features").as_deref(), Some("3"));
    assert_eq!(property(&h, "Saved").as_deref(), Some("Yes"));
    assert_eq!(property(&h, "Bodies").as_deref(), Some("1"));
    for section in ["PROPERTIES", "DOCUMENT", "MODEL"] {
        assert!(shows(&h, "Properties", section), "{section}");
    }
    // A feature selected: its properties instead; Esc clears it, and the document's are back.
    select(&mut h, "Hole");
    assert_eq!(property(&h, "Revision"), None);
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the document's again", |h| {
        property(h, "Revision").as_deref() == Some("1")
    });
    // An edit of the pin's name: unsaved, at a later revision, the model's the one built.
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| anywhere(h, "example.silico *"));
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the edit shown", |h| {
        property(h, "Saved").as_deref() == Some("Unsaved edits")
            && property(h, "Revision").is_some_and(|said| said != "1")
    });
    // Another program writes the file: changed on disk, said in the panel and the bar.
    std::fs::write(&path, EXAMPLE.replace("Clearance", "Room")).unwrap();
    step_until(&mut h, "the change found", |h| {
        property(h, "File on disk").as_deref() == Some("Changed by another program")
    });
    assert!(
        h.query_by_role_and_label(Role::Button, "Changed on disk")
            .is_some()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_selected_features_properties_are_its_name_health_and_id() {
    let f = fixture();
    f.kernel.refuse(|op| {
        matches!(op, Op::Boolean { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "the cut came apart"))
    });
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    // The pin built, warned of its missing fingerprint (the document stores none:
    // `fingerprint_missing`).
    select(&mut h, "Pin");
    assert_eq!(property(&h, "State").as_deref(), Some("Warning"));
    assert_eq!(property(&h, "ID").as_deref(), Some("a91cw0de"));
    assert!(shows(&h, "Properties", "HEALTH"));
    // The failed cut: its health, and its error's text in its card, as the core's check
    // says it.
    select(&mut h, "Hole");
    assert_eq!(property(&h, "State").as_deref(), Some("Error"));
    assert_eq!(property(&h, "ID").as_deref(), Some("m3tq8v0z"));
    let want = core_says(&f, EXAMPLE, "m3tq8v0z");
    step_until(&mut h, "its error's text", |h| {
        shows(h, "Properties", &want)
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_open_dialog_takes_the_property_panels_place_and_cancel_gives_it_back() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    // The dialog is in the panel (one right panel), the document's properties gone.
    let panel = h.get_by_role_and_label(Role::Pane, "Properties");
    assert!(panel.query_by_role_and_label(Role::Dialog, "Box").is_some());
    assert!(panel.query_by_label("INPUTS").is_some());
    assert_eq!(property(&h, "Revision"), None);
    dialog(&h)
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    step_until(&mut h, "the document's properties", |h| {
        property(h, "Revision").is_some()
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_status_bar_says_the_rebuild_running_stopped_and_rebuilt_with_stop_and_rebuild() {
    let f = fixture();
    let hold = f.kernel.hold(is_box);
    let path = f.file("example.silico", EXAMPLE);
    let mut h = f.app();
    h.state_mut().open(path.to_str().unwrap(), false);
    // Running: how far it's got, as `status` counts its plan's steps.
    let progress = |h: &AppHarness| {
        h.state()
            .tabs()
            .first()
            .and_then(|tab| tab.status.as_ref())
            .and_then(|status| status.rebuild.progress.clone())
    };
    step_until(&mut h, "the rebuild's progress", |h| {
        progress(h).is_some_and(|p| {
            status_said(h, "Rebuilding").as_deref()
                == Some(format!("Rebuilding · {} of {}", p.done, p.steps).as_str())
        })
    });
    let stop = h.get_by_role_and_label(Role::Button, "Stop");
    assert!(!stop.accesskit_node().is_disabled());
    assert!(
        h.get_by_role_and_label(Role::Button, "Rebuild")
            .accesskit_node()
            .is_disabled()
    );
    // Stopped: said so, as far as it got, and Rebuild on.
    h.get_by_role_and_label(Role::Button, "Stop").click();
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    step_until(&mut h, "the stop said", |h| {
        progress(h).is_some_and(|p| {
            status_said(h, "Rebuild stopped").as_deref()
                == Some(format!("Rebuild stopped at {} of {}", p.done, p.steps).as_str())
        })
    });
    assert!(
        h.get_by_role_and_label(Role::Button, "Stop")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        !h.get_by_role_and_label(Role::Button, "Rebuild")
            .accesskit_node()
            .is_disabled()
    );
    hold.release();
    h.get_by_role_and_label(Role::Button, "Rebuild").click();
    // Rebuilt: revision 1, the one it opened at, with no error.
    step_until(&mut h, "rebuilt", |h| {
        status_said(h, "Rebuilt").as_deref() == Some("Rebuilt · revision 1")
    });
    assert!(
        h.get_by_role_and_label(Role::Button, "Rebuild")
            .accesskit_node()
            .is_disabled()
    );
    assert!(status_said(&h, "Saved").is_some());
    assert!(status_said(&h, "mm").is_some());
    assert!(
        h.query_by_role_and_label(Role::Button, "Shortcuts")
            .is_some()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_status_bar_says_a_rebuild_with_errors_and_an_edit_unsaved() {
    let f = fixture();
    f.kernel.refuse(|op| {
        matches!(op, Op::Boolean { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "the cut came apart"))
    });
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    // The cut failed: one error, the document's one problem.
    step_until(&mut h, "rebuilt with its error", |h| {
        status_said(h, "Rebuilt").as_deref() == Some("Rebuilt with 1 error")
    });
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "unsaved", |h| {
        status_said(h, "Unsaved").as_deref() == Some("Unsaved")
    });
    assert!(status_said(&h, "Saved").is_none());
    // A dialog open: its edits are its own until OK.
    create(&mut h, "Box");
    step_until(&mut h, "the dialog said", |h| {
        status_said(h, "Unsaved").as_deref() == Some("Unsaved · dialog open")
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_changed_on_disk_chip_offers_again_the_choice_the_edits_were_kept_over() {
    let f = fixture_reading(Some(Arc::new(real)));
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    assert!(
        h.query_by_role_and_label(Role::Button, "Changed on disk")
            .is_none()
    );
    select(&mut h, "Pin");
    type_name(&mut h, "s");
    step_until(&mut h, "the edit held", |h| anywhere(h, "example.silico *"));
    std::fs::write(&path, EXAMPLE.replace("Clearance", "Room")).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| anywhere(h, said));
    h.get_by_role_and_label(Role::Button, "Keep my edits…")
        .click();
    step_until(&mut h, "the banner gone", |h| !anywhere(h, said));
    // The chip stays while the file differs; a click on it offers the choice again.
    let chip = h.get_by_role_and_label(Role::Button, "Changed on disk");
    chip.click();
    step_until(&mut h, "the choice offered again", |h| anywhere(h, said));
    assert!(
        h.query_by_role_and_label(Role::Button, "Reload, discard my edits")
            .is_some()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_dialogs_close_button_cancels_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    assert!(shows(&h, "Timeline", "Box"));
    dialog(&h)
        .get_by_role_and_label(Role::Button, "Close the dialog")
        .click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    // Cancel's: the box it added is taken away again.
    step_until(&mut h, "the box gone", |h| {
        shows(h, "Timeline", "Hole") && !shows(h, "Timeline", "Box")
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_dialogs_preview_line_says_its_rebuild_running_stopped_and_with_errors() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let hold = f.kernel.hold(is_box);
    create(&mut h, "Box");
    added(&mut h);
    // Running: what stops it said beside it.
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
            && dialog(h)
                .query_by_label_contains("Rebuilding the preview")
                .is_some()
    });
    assert!(dialog(&h).query_by_label("Esc stops it").is_some());
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the preview stopped", |h| {
        dialog(h)
            .query_by_label_contains("Preview stopped. Rebuild ")
            .is_some()
    });
    assert!(dialog_open(&h));
    hold.release();
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    // A box the kernel refuses: the preview's error, and that OK keeps the feature with it.
    f.kernel
        .refuse(|op| is_box(op).then(|| Error::new(ErrorCode::KernelFailed, "the box came apart")));
    create(&mut h, "Box");
    added(&mut h);
    step_until(&mut h, "the preview's errors", |h| {
        dialog(h)
            .query_by_label_contains("OK keeps the feature with them.")
            .is_some_and(|node| {
                node.accesskit_node()
                    .label()
                    .or_else(|| node.accesskit_node().value())
                    .is_some_and(|said| said.starts_with("The preview has "))
            })
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn shortcuts_opens_the_shortcut_sheet() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let shortcuts = h.get_by_role_and_label(Role::Button, "Shortcuts");
    assert!(!shortcuts.accesskit_node().is_disabled());
    shortcuts.click();
    step_until(&mut h, "the sheet", sheet_open);
    assert_eq!(never_waits::waits(), 0);
}

// ---- The keyboard: one registry for the keys, the sheet, and the tooltips ----

use eframe::egui::{Key, Modifiers};
use silico_app::shortcuts::{Chord, Command, REGISTRY};

/// The shortcut sheet, while it's open.
fn the_sheet(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.get_by_role_and_label(Role::Dialog, "Keyboard shortcuts")
}

/// Whether the shortcut sheet holds a node labelled `label` (a label's text, too).
fn sheet_shows(h: &AppHarness, label: &str) -> bool {
    the_sheet(h).query_all_by_label(label).next().is_some()
}

/// Whether the shortcut sheet is open.
fn sheet_open(h: &AppHarness) -> bool {
    h.query_by_role_and_label(Role::Dialog, "Keyboard shortcuts")
        .is_some()
}

/// The keyboard taken from every text field, by a click on the words labelled `words`:
/// what a key does then is no field's.
fn off_fields(h: &mut AppHarness, words: &str) {
    step(h);
    h.get_by_label(words).click();
    step(h);
    step(h);
    assert!(!h.ctx.text_edit_focused(), "a field still has the keyboard");
}

/// `chord` pressed, as the registry gives it.
fn press_chord(h: &mut AppHarness, chord: Chord) {
    match chord {
        Chord::Key(shortcut) | Chord::Exact(shortcut) => {
            h.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        }
        Chord::Pointer(..) => panic!("{chord:?} is the pointer's"),
    }
}

/// `command`'s chords, each a key's.
fn keys_of(command: Command) -> Vec<Chord> {
    let keys = command.entry().keys.to_vec();
    assert!(!keys.is_empty(), "{command:?} has no keys");
    keys
}

#[test]
fn question_mark_opens_the_sheet_its_filter_narrows_it_and_esc_closes_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    h.key_press(Key::Questionmark);
    step_until(&mut h, "the sheet", sheet_open);
    // Every command with keys, each under its group, with its keys as the platform names
    // them (egui_kittest's is neither macOS nor Windows: Ctrl).
    for (label, keys) in [
        ("New part", "Ctrl+N"),
        ("Redo", "Ctrl+Y"),
        ("Rebuild everything", "Ctrl+Shift+B"),
        ("Stop rebuild", "Esc"),
        ("Fit", "F6"),
        ("Home view", "Home"),
        ("Front", "Ctrl+1"),
        ("Isometric", "Ctrl+7"),
        ("Orbit", "Shift+Middle-drag"),
        ("Hide or show", "H"),
        ("OK", "↵"),
        ("Keyboard shortcuts", "?"),
    ] {
        assert!(sheet_shows(&h, label), "{label}");
        assert!(sheet_shows(&h, keys), "{label}'s {keys}");
    }
    for group in [
        "FILE",
        "EDIT",
        "REBUILD",
        "VIEW",
        "SELECTION",
        "DIALOGS",
        "HELP",
    ] {
        assert!(sheet_shows(&h, group), "{group}");
    }
    // The filter has the keyboard: what's typed narrows the sheet, by a key too.
    let filter = |h: &AppHarness| {
        h.get_by_role_and_label(Role::TextInput, "Filter shortcuts")
            .accesskit_node()
            .is_focused()
    };
    step_until(&mut h, "the filter focused", filter);
    h.get_by_role_and_label(Role::TextInput, "Filter shortcuts")
        .type_text("f6");
    step_until(&mut h, "narrowed to Fit", |h| {
        sheet_shows(h, "Fit") && !sheet_shows(h, "Save")
    });
    assert!(sheet_shows(&h, "VIEW"));
    assert!(!sheet_shows(&h, "FILE"));
    // A ? typed in the filter is the filter's.
    h.get_by_role_and_label(Role::TextInput, "Filter shortcuts")
        .type_text("?");
    step(&mut h);
    assert!(sheet_open(&h));
    // Esc closes it, though its filter has the keyboard; ? opens it again, its filter
    // empty, and a name narrows it to its commands.
    h.key_press(Key::Escape);
    step_until(&mut h, "the sheet closed", |h| !sheet_open(h));
    step(&mut h);
    h.key_press(Key::Questionmark);
    step_until(&mut h, "the sheet again", sheet_open);
    assert!(sheet_shows(&h, "New part"));
    step_until(&mut h, "the filter focused", filter);
    h.get_by_role_and_label(Role::TextInput, "Filter shortcuts")
        .type_text("save");
    step_until(&mut h, "narrowed to Save", |h| {
        sheet_shows(h, "Save") && sheet_shows(h, "Save As…") && !sheet_shows(h, "Fit")
    });
    // Nothing found: said.
    h.key_press(Key::Escape);
    step_until(&mut h, "the sheet closed", |h| !sheet_open(h));
    step(&mut h);
    h.key_press(Key::Questionmark);
    step_until(&mut h, "the sheet again", sheet_open);
    step_until(&mut h, "the filter focused", filter);
    h.get_by_role_and_label(Role::TextInput, "Filter shortcuts")
        .type_text("zzz");
    step_until(&mut h, "nothing found", |h| {
        sheet_shows(h, "No shortcut matches.")
    });
    // Its × closes it too.
    the_sheet(&h)
        .get_by_role_and_label(Role::Button, "Close")
        .click();
    step_until(&mut h, "the sheet closed", |h| !sheet_open(h));
    assert_eq!(h.state().tabs().len(), 1, "the ×, not File's Close");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn question_mark_opens_the_sheet_on_the_start_page_but_not_while_typing() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    h.key_press(Key::Questionmark);
    step_until(&mut h, "the sheet", sheet_open);
    h.key_press(Key::Escape);
    step_until(&mut h, "the sheet closed", |h| !sheet_open(h));
    // Typing in a field: the ? is the field's.
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    name_field(&h).focus();
    step(&mut h);
    h.key_press(Key::Questionmark);
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(!sheet_open(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn under_the_sheet_no_shortcut_acts() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    let path = f.dir.path().join("example.silico");
    meshes_settled(&mut h);
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    // Off the field, the sheet opened, and off its filter.
    h.get_by_role_and_label(Role::Button, "Shortcuts").click();
    step_until(&mut h, "the sheet", sheet_open);
    off_fields(&mut h, "FILE");
    let camera = view(&h).camera().clone();
    let boxes = f
        .kernel
        .calls()
        .iter()
        .filter(|call| is_box(&call.op))
        .count();
    for (modifiers, key) in [
        (Modifiers::COMMAND, Key::S),
        (Modifiers::COMMAND, Key::Z),
        (Modifiers::COMMAND, Key::W),
        (Modifiers::COMMAND | Modifiers::SHIFT, Key::B),
        (Modifiers::NONE, Key::F6),
        (Modifiers::COMMAND, Key::Num1),
        (Modifiers::NONE, Key::H),
        (Modifiers::COMMAND, Key::N),
    ] {
        h.key_press_modifiers(modifiers, key);
        step(&mut h);
    }
    for _ in 0..5 {
        step(&mut h);
    }
    let tab = &h.state().tabs()[0];
    assert_eq!((tab.saves, tab.rebuilds_all), (0, 0));
    assert!(tab.hidden.is_empty());
    assert!(anywhere(&h, "example.silico *"));
    assert_eq!(view(&h).camera(), &camera);
    assert_eq!(
        h.state().tabs().len(),
        1,
        "Ctrl+N made a part under the sheet"
    );
    assert_eq!(
        f.kernel
            .calls()
            .iter()
            .filter(|call| is_box(&call.op))
            .count(),
        boxes
    );
    assert_eq!(names_in(&path), ["Block", "Pin", "Hole"]);
    assert!(sheet_open(&h));
    assert_eq!(never_waits::waits(), 0);
}

/// The unit vector of `v`.
fn unit(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / n, v[1] / n, v[2] / n]
}

/// The view moved off where `camera` looks, as a pan does: its eye and target 7 along x.
fn panned(h: &mut AppHarness) {
    let mut camera = view(h).camera().clone();
    camera.eye[0] += 7.0;
    camera.target[0] += 7.0;
    view_mut(h).set_camera(camera);
    step(h);
}

#[test]
fn ctrl_1_to_7_are_solidworks_standard_views_and_each_is_the_view_cubes() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    let d = 1.0 / 3f64.sqrt();
    // docs/design/m5-ui/DESIGN.md, *Keyboard*, and js/app.js's `CMD` (SolidWorks'
    // numbering): Ctrl+1 front, 2 back, 3 left, 4 right, 5 top, 6 bottom, 7 isometric;
    // each the direction from the target to the eye, Z up, by hand.
    for (digit, cube, toward) in [
        (Key::Num1, "Front", [0.0, -1.0, 0.0]),
        (Key::Num2, "Back", [0.0, 1.0, 0.0]),
        (Key::Num3, "Left", [-1.0, 0.0, 0.0]),
        (Key::Num4, "Right", [1.0, 0.0, 0.0]),
        (Key::Num5, "Top", [0.0, 0.0, 1.0]),
        (Key::Num6, "Bottom", [0.0, 0.0, -1.0]),
        (Key::Num7, "Top front right", [d, -d, d]),
    ] {
        h.key_press_modifiers(Modifiers::COMMAND, digit);
        step(&mut h);
        let keyed = view(&h).camera().clone();
        near(unit(minus(keyed.eye, keyed.target)), toward, 1e-12);
        // The cut's box's centre, as every named view fits it.
        near(keyed.target, [20.0, 10.0, 5.0], 1e-12);
        // Moved off, then the ViewCube's own: the same camera.
        panned(&mut h);
        assert_ne!(view(&h).camera(), &keyed);
        h.get_by_role_and_label(Role::Group, "ViewCube")
            .get_by_label(cube)
            .click();
        step(&mut h);
        assert_eq!(view(&h).camera(), &keyed, "{cube}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn f6_is_the_navigation_bars_fit_and_home_the_view_cubes_home() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step(&mut h);
    panned(&mut h);
    let moved = view(&h).camera().clone();
    h.key_press(Key::F6);
    step(&mut h);
    let keyed = view(&h).camera().clone();
    assert_ne!(keyed, moved);
    // Fit looks as it looked: from the front still, onto the box's centre.
    near(
        unit(minus(keyed.eye, keyed.target)),
        [0.0, -1.0, 0.0],
        1e-12,
    );
    near(keyed.target, [20.0, 10.0, 5.0], 1e-12);
    view_mut(&mut h).set_camera(moved);
    step(&mut h);
    h.get_by_role_and_label(Role::Toolbar, "Navigation bar")
        .get_by_label("Fit")
        .click();
    step(&mut h);
    assert_eq!(view(&h).camera(), &keyed);
    // Home: the corner's view, as the ViewCube's Home gives it.
    h.key_press(Key::Home);
    step(&mut h);
    let home = view(&h).camera().clone();
    let d = 1.0 / 3f64.sqrt();
    near(unit(minus(home.eye, home.target)), [d, -d, d], 1e-12);
    panned(&mut h);
    h.get_by_role_and_label(Role::Group, "ViewCube")
        .get_by_label("Home")
        .click();
    step(&mut h);
    assert_eq!(view(&h).camera(), &home);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_view_keys_do_nothing_while_a_field_is_typed_in_or_a_modal_is_open() {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step(&mut h);
    panned(&mut h);
    let camera = view(&h).camera().clone();
    let keys = [
        (Modifiers::NONE, Key::F6),
        (Modifiers::NONE, Key::Home),
        (Modifiers::COMMAND, Key::Num1),
        (Modifiers::COMMAND, Key::Num2),
        (Modifiers::COMMAND, Key::Num3),
        (Modifiers::COMMAND, Key::Num4),
        (Modifiers::COMMAND, Key::Num5),
        (Modifiers::COMMAND, Key::Num6),
        (Modifiers::COMMAND, Key::Num7),
    ];
    // Typing in the Name field: each key the field's.
    select(&mut h, "Pin");
    name_field(&h).focus();
    step(&mut h);
    for (modifiers, key) in keys {
        h.key_press_modifiers(modifiers, key);
        step(&mut h);
        assert_eq!(view(&h).camera(), &camera, "{key:?} while typing");
    }
    // Home is the field's own: its cursor to its start, so what's typed goes there.
    h.key_press(Key::End);
    step(&mut h);
    h.key_press(Key::Home);
    step(&mut h);
    name_field(&h).type_text("Z");
    step_until(&mut h, "typed at the field's start", |h| {
        name_field(h).value().as_deref() == Some("ZPin")
    });
    // A modal open (Save As's): each key the modal's, the view still.
    h.key_press(Key::Escape);
    step(&mut h);
    settled(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
    step_until(&mut h, "the Save As dialog", |h| {
        anywhere(h, "Save example.silico as")
    });
    // Off its field (a click on its words), so the keys aren't a field's either.
    off_fields(&mut h, "Save example.silico as");
    let camera = view(&h).camera().clone();
    for (modifiers, key) in keys {
        h.key_press_modifiers(modifiers, key);
        step(&mut h);
        assert_eq!(view(&h).camera(), &camera, "{key:?} under a modal");
    }
    assert!(anywhere(&h, "Save example.silico as"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_shift_b_rebuilds_every_feature_anew_and_esc_stops_it() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "rebuilt", |h| {
        status_said(h, "Rebuilt").as_deref() == Some("Rebuilt · revision 1")
    });
    settled(&mut h);
    let boxes = |f: &Fixture| {
        f.kernel
            .calls()
            .iter()
            .filter(|call| is_box(&call.op))
            .count()
    };
    assert_eq!(boxes(&f), 1);
    // Ctrl+B with nothing stopped: nothing rebuilt.
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(boxes(&f), 1);
    assert_eq!(h.state().tabs()[0].rebuilds, 0);
    // Ctrl+Shift+B: every step built again, the box too, though the session and the cache
    // hold it; the same model, at the same revision.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::B);
    step_until(&mut h, "the box made again", |h| {
        boxes(&f) == 2 && !h.state().tabs()[0].checking()
    });
    assert_eq!(h.state().tabs()[0].rebuilds_all, 1);
    assert_eq!(h.state().tabs()[0].rebuilds, 0, "never a Rebuild");
    step_until(&mut h, "rebuilt again", |h| {
        status_said(h, "Rebuilt").as_deref() == Some("Rebuilt · revision 1")
    });
    assert!(shows(&h, "Timeline", "Hole"));
    // Held at its box: Esc stops it, as Stop does, and the revision keeps the model it
    // had (`status` says it's rebuilt, of that model): nothing more is built.
    let hold = f.kernel.hold(is_box);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::B);
    step(&mut h);
    assert!(f.kernel.wait_running(1, WAIT));
    step_until(&mut h, "rebuilding", |h| {
        status_said(h, "Rebuilding").is_some()
    });
    assert_eq!(boxes(&f), 3);
    h.key_press(Key::Escape);
    step_until(&mut h, "the fresh rebuild stopped", |h| {
        !h.state().tabs()[0].checking()
            && status_said(h, "Rebuilt").as_deref() == Some("Rebuilt · revision 1")
    });
    assert!(h.state().tabs()[0].message.is_none());
    let calls = f.kernel.calls().len();
    hold.release();
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(f.kernel.wait_idle(WAIT));
    assert_eq!(
        f.kernel.calls().len(),
        calls,
        "nothing asked of the kernel after the stop"
    );
    assert_eq!(h.state().tabs()[0].rebuilds_all, 2);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_shift_b_does_nothing_while_a_field_is_typed_in_or_a_modal_is_open() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    let boxes = |f: &Fixture| {
        f.kernel
            .calls()
            .iter()
            .filter(|call| is_box(&call.op))
            .count()
    };
    // A rebuild stopped, so a Ctrl+B would rebuild: a Box dialog's preview, held at its
    // new box, stopped by Esc, the dialog left open.
    let hold = f.kernel.hold(is_box);
    create(&mut h, "Box");
    step_until(&mut h, "the preview rebuilding", |h| {
        h.state().tabs()[0].preview_rebuilding()
    });
    h.key_press(Key::Escape);
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    hold.release();
    assert!(dialog_open(&h));
    assert!(h.state().tabs()[0].can_rebuild());
    let built = boxes(&f);
    // Typing in one of its fields: neither everything rebuilt nor a Rebuild, which egui
    // would match Ctrl+Shift+B as (its Shift aside).
    dialog(&h)
        .query_all_by_role(Role::TextInput)
        .next()
        .unwrap()
        .focus();
    step(&mut h);
    assert!(h.ctx.text_edit_focused());
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::B);
    for _ in 0..5 {
        step(&mut h);
    }
    let tab = &h.state().tabs()[0];
    assert_eq!((tab.rebuilds_all, tab.rebuilds), (0, 0));
    assert!(rebuild_stopped(&h), "still stopped");
    assert_eq!(boxes(&f), built);
    // The dialog cancelled; a modal open (the typed Open's, with no native dialogs), off
    // its field.
    h.key_press(Key::Escape);
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::O);
    step_until(&mut h, "Open's question", |h| anywhere(h, "Open document"));
    off_fields(&mut h, "Open document");
    let built = boxes(&f);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::B);
    for _ in 0..5 {
        step(&mut h);
    }
    assert_eq!(h.state().tabs()[0].rebuilds_all, 0);
    assert_eq!(boxes(&f), built);
    assert_eq!(never_waits::waits(), 0);
}

// ---- Each command's keys do what its button or menu item does ----

/// A question's modal (Open's, Save As's) asked by `chord`, then by `click`: each
/// asks `title`.
fn same_question(h: &mut AppHarness, chord: Chord, title: &str, click: impl Fn(&mut AppHarness)) {
    press_chord(h, chord);
    step_until(h, title, |h| anywhere(h, title));
    h.key_press(Key::Escape);
    step_until(h, "the question gone", |h| !anywhere(h, title));
    click(h);
    step_until(h, title, |h| anywhere(h, title));
    h.key_press(Key::Escape);
    step_until(h, "the question gone", |h| !anywhere(h, title));
}

/// The File menu's `item` clicked.
fn from_file_menu(h: &mut AppHarness, item: &str) {
    h.get_by_label("File").click();
    step(h);
    button_off_start(h, item).click();
    step(h);
}

/// The name typed into Pin's field, its edit landed.
fn renamed(h: &mut AppHarness, typed: &str, name: &str) {
    type_name(h, typed);
    step_until(h, "the edit landed", |h| {
        h.state().tabs()[0].edits.idle() && shows(h, "Timeline", name)
    });
}

/// A document opened with its rebuild held at its box, stopped by `stop`, then resumed by
/// `resume` once the box may be made: what each leaves.
fn stopped_then_resumed(stop: impl Fn(&mut AppHarness), resume: impl Fn(&mut AppHarness)) {
    let f = fixture();
    let hold = f.kernel.hold(is_box);
    let path = f.file("example.silico", EXAMPLE);
    let mut h = f.app();
    h.state_mut().open(path.to_str().unwrap(), false);
    step_until(&mut h, "rebuilding", |h| {
        status_said(h, "Rebuilding").is_some()
    });
    stop(&mut h);
    step_until(&mut h, "the rebuild stopped", rebuild_stopped);
    hold.release();
    resume(&mut h);
    step_until(&mut h, "rebuilt", |h| {
        status_said(h, "Rebuilt").as_deref() == Some("Rebuilt · revision 1")
    });
    assert_eq!(h.state().tabs()[0].rebuilds, 1);
}

/// A view command's keys and its ViewCube part (or the navigation bar's Fit): from a view
/// moved off, the same camera.
fn same_view(command: Command, part: impl Fn(&AppHarness) -> egui_kittest::Node<'_>) {
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    for chord in keys_of(command) {
        // Looked at from the view the command gives (so its cube part faces the viewer),
        // then moved off; from there, by its keys, and from there again, by its part.
        press_chord(&mut h, chord);
        step(&mut h);
        panned(&mut h);
        let moved = view(&h).camera().clone();
        press_chord(&mut h, chord);
        step(&mut h);
        let keyed = view(&h).camera().clone();
        assert_ne!(keyed, moved, "{command:?} moved nothing");
        view_mut(&mut h).set_camera(moved);
        step(&mut h);
        part(&h).click();
        step(&mut h);
        assert_eq!(view(&h).camera(), &keyed, "{command:?}");
    }
}

#[test]
fn every_registry_commands_keys_do_what_its_button_or_menu_item_does() {
    for entry in REGISTRY {
        let command = entry.command;
        match command {
            Command::New => {
                // An untitled part at once, by its keys and by the start page's own button,
                // labelled as the registry names the command: each from a fresh app, so
                // each is the run's first, "Untitled".
                let untitled = |ask: &dyn Fn(&mut AppHarness)| {
                    let f = fixture();
                    let mut h = f.app();
                    step(&mut h);
                    ask(&mut h);
                    step_until(&mut h, "the untitled part's tab", |h| {
                        h.state().tabs().len() == 1
                    });
                    let tab = &h.state().tabs()[0];
                    assert_eq!((tab.name(), tab.path.as_deref()), ("Untitled", None));
                };
                for chord in keys_of(command) {
                    untitled(&|h| press_chord(h, chord));
                }
                untitled(&|h| {
                    start_page(h).get_by_label(entry.label).click();
                    step(h);
                });
            }
            Command::Open => {
                // The start page's own, labelled as the registry names the command.
                let f = fixture();
                let mut h = f.app();
                step(&mut h);
                for chord in keys_of(command) {
                    same_question(&mut h, chord, "Open document", |h| {
                        start_page(h).get_by_label(entry.label).click();
                        step(h);
                    });
                }
            }
            Command::SaveAs => {
                let f = fixture();
                let (mut h, _) = f.opened("example.silico", EXAMPLE);
                step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
                for chord in keys_of(command) {
                    same_question(&mut h, chord, "Save example.silico as", |h| {
                        from_file_menu(h, entry.label);
                    });
                }
            }
            Command::Close => {
                let f = fixture();
                let (mut h, path) = f.opened("example.silico", EXAMPLE);
                for chord in keys_of(command) {
                    press_chord(&mut h, chord);
                    step_until(&mut h, "closed by its keys", |h| {
                        h.state().tabs().is_empty()
                    });
                    h.state_mut().open(path.to_str().unwrap(), false);
                    step_until(&mut h, "open again", |h| h.state().tabs().len() == 1);
                    from_file_menu(&mut h, entry.label);
                    step_until(&mut h, "closed by File", |h| h.state().tabs().is_empty());
                }
            }
            Command::Save => {
                let f = fixture();
                let (mut h, path) = f.opened("example.silico", EXAMPLE);
                step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
                select(&mut h, "Pin");
                for chord in keys_of(command) {
                    renamed(&mut h, "K", "PinK");
                    press_chord(&mut h, chord);
                    step_until(&mut h, "saved by its keys", |h| {
                        !anywhere(h, "example.silico *")
                    });
                    assert_eq!(names_in(&path), ["Block", "PinK", "Hole"]);
                    renamed(&mut h, "L", "PinKL");
                    h.get_by_role_and_label(Role::Button, entry.label).click();
                    step_until(&mut h, "saved by its button", |h| {
                        !anywhere(h, "example.silico *")
                    });
                    assert_eq!(names_in(&path), ["Block", "PinKL", "Hole"]);
                }
            }
            Command::Undo | Command::Redo => {
                let f = fixture();
                let (mut h, _) = f.opened("example.silico", EXAMPLE);
                step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
                select(&mut h, "Pin");
                renamed(&mut h, "A", "PinA");
                let undo = command == Command::Undo;
                for chord in keys_of(command) {
                    // Undo's from PinA to Pin, Redo's from Pin to PinA: by its keys, then
                    // by its button, the other's button between.
                    let (from, to, other) = if undo {
                        ("PinA", "Pin", "Redo")
                    } else {
                        ("Pin", "PinA", "Undo")
                    };
                    if !undo {
                        h.get_by_role_and_label(Role::Button, other).click();
                        step_until(&mut h, from, |h| shows(h, "Timeline", from));
                    }
                    press_chord(&mut h, chord);
                    step_until(&mut h, "by its keys", |h| shows(h, "Timeline", to));
                    h.get_by_role_and_label(Role::Button, other).click();
                    step_until(&mut h, from, |h| shows(h, "Timeline", from));
                    h.get_by_role_and_label(Role::Button, entry.label).click();
                    step_until(&mut h, "by its button", |h| shows(h, "Timeline", to));
                    if undo {
                        h.get_by_role_and_label(Role::Button, other).click();
                        step_until(&mut h, from, |h| shows(h, "Timeline", from));
                    }
                }
            }
            Command::Stop => {
                for chord in keys_of(command) {
                    stopped_then_resumed(
                        |h| press_chord(h, chord),
                        |h| h.get_by_role_and_label(Role::Button, "Rebuild").click(),
                    );
                }
                stopped_then_resumed(
                    |h| h.get_by_role_and_label(Role::Button, "Stop").click(),
                    |h| h.get_by_role_and_label(Role::Button, "Rebuild").click(),
                );
            }
            Command::Rebuild => {
                for chord in keys_of(command) {
                    stopped_then_resumed(
                        |h| h.get_by_role_and_label(Role::Button, "Stop").click(),
                        |h| press_chord(h, chord),
                    );
                }
            }
            // Its own test: no button or menu item has it
            // (ctrl_shift_b_rebuilds_every_feature_anew_and_esc_stops_it).
            Command::RebuildAll => assert_eq!(
                keys_of(command),
                [Chord::Key(eframe::egui::KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::B
                ))]
            ),
            Command::Fit => same_view(command, |h| {
                h.get_by_role_and_label(Role::Toolbar, "Navigation bar")
                    .get_by_label(entry.label)
            }),
            Command::Home => same_view(command, |h| {
                h.get_by_role_and_label(Role::Group, "ViewCube")
                    .get_by_label("Home")
            }),
            Command::Front
            | Command::Back
            | Command::Left
            | Command::Right
            | Command::Top
            | Command::Bottom => same_view(command, |h| {
                h.get_by_role_and_label(Role::Group, "ViewCube")
                    .get_by_label(entry.label)
            }),
            Command::Isometric => same_view(command, |h| {
                h.get_by_role_and_label(Role::Group, "ViewCube")
                    .get_by_label("Top front right")
            }),
            // The pointer's: the viewport takes them, no key (their own tests: the middle
            // button's pan and orbit, the wheel's zoom).
            Command::WhatsWrong => {
                // No keys: the status bar's errors open it, a button whose tooltip is the
                // registry's (What's Wrong's own test covers the timeline and the browser).
                assert!(entry.keys.is_empty());
                let f = fixture();
                let mut h = three_blocks(&f);
                h.get_by_role_and_label(Role::Button, "Rebuilt with 2 errors")
                    .click();
                step_until(&mut h, "What's Wrong open", |h| whats_wrong(h).is_some());
            }
            Command::Orbit | Command::Pan | Command::Zoom => assert!(
                entry
                    .keys
                    .iter()
                    .all(|chord| matches!(chord, Chord::Pointer(..)))
            ),
            Command::ClearSelection => {
                let f = fixture();
                let (mut h, _) = f.opened("example.silico", EXAMPLE);
                step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
                for chord in keys_of(command) {
                    select(&mut h, "Pin");
                    pointer_away(&mut h);
                    press_chord(&mut h, chord);
                    step(&mut h);
                    assert!(h.state().tabs()[0].selection().is_empty(), "by its keys");
                    select(&mut h, "Pin");
                    pointer_away(&mut h);
                    h.get_by_role_and_label(Role::Toolbar, "SELECT")
                        .get_by_role_and_label(Role::Button, "SELECT")
                        .click();
                    step_until(&mut h, "SELECT's drop-down", |h| {
                        h.query_by_role_and_label(Role::Menu, "SELECT").is_some()
                    });
                    step(&mut h);
                    h.get_by_role_and_label(Role::Menu, "SELECT")
                        .query_all_by_role(Role::Button)
                        .find(|node| {
                            node.accesskit_node()
                                .label()
                                .is_some_and(|label| label.starts_with(entry.label))
                        })
                        .unwrap()
                        .click();
                    step(&mut h);
                    step(&mut h);
                    assert!(h.state().tabs()[0].selection().is_empty(), "by SELECT's");
                }
            }
            Command::Hide => {
                let f = fixture();
                let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES);
                meshes_settled(&mut h);
                step_until(&mut h, "the browser's bodies", |h| {
                    shows(h, "Browser", "Large")
                });
                browser(&h).get_by_label("Large").click();
                step_until(&mut h, "Large selected", |h| {
                    toggled(&browser(h).get_by_label("Large")) == Some(true)
                });
                let hidden = |h: &AppHarness| h.state().tabs()[0].hidden.clone();
                for chord in keys_of(command) {
                    // By its keys, hidden and shown again; then by Large's eye, the same.
                    press_chord(&mut h, chord);
                    step(&mut h);
                    step(&mut h);
                    assert!(shows(&h, "Browser", "Show Large"), "hidden by its keys");
                    let by_keys = hidden(&h);
                    press_chord(&mut h, chord);
                    step(&mut h);
                    step(&mut h);
                    assert!(shows(&h, "Browser", "Hide Large"), "shown by its keys");
                    assert!(hidden(&h).is_empty());
                    browser(&h).get_by_label("Hide Large").click();
                    step(&mut h);
                    step(&mut h);
                    assert!(shows(&h, "Browser", "Show Large"), "hidden by its eye");
                    assert_eq!(hidden(&h), by_keys);
                    browser(&h).get_by_label("Show Large").click();
                    step(&mut h);
                    step(&mut h);
                    assert!(hidden(&h).is_empty());
                }
            }
            Command::FilterFace | Command::FilterEdge | Command::FilterVertex => {
                // Its key toggles its kind in the filter, the others kept, and back again;
                // so does SELECT's button, pressed while the kind is taken (M5.0, open item
                // 24: each a toggle in the set).
                let f = fixture();
                let (mut h, _) = f.opened("example.silico", EXAMPLE);
                step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
                let filter = |h: &AppHarness| h.state().tabs()[0].selection().filter;
                let kinds = |h: &AppHarness| {
                    let f = filter(h);
                    (f.faces, f.edges, f.vertices)
                };
                let off = match command {
                    Command::FilterFace => (false, true, true),
                    Command::FilterEdge => (true, false, true),
                    _ => (true, true, false),
                };
                let pressed = |h: &AppHarness| {
                    toggled(
                        &h.get_by_role_and_label(Role::Toolbar, "SELECT")
                            .get_by_label(entry.label),
                    )
                };
                for chord in keys_of(command) {
                    assert_eq!(kinds(&h), (true, true, true));
                    pointer_away(&mut h);
                    press_chord(&mut h, chord);
                    step(&mut h);
                    step(&mut h);
                    assert_eq!(kinds(&h), off, "{command:?} by its key");
                    assert_eq!(pressed(&h), Some(false));
                    press_chord(&mut h, chord);
                    step(&mut h);
                    step(&mut h);
                    assert_eq!(kinds(&h), (true, true, true), "and back");
                    assert_eq!(pressed(&h), Some(true));
                    h.get_by_role_and_label(Role::Toolbar, "SELECT")
                        .get_by_label(entry.label)
                        .click();
                    step(&mut h);
                    step(&mut h);
                    assert_eq!(kinds(&h), off, "{command:?} by SELECT's button");
                    h.get_by_role_and_label(Role::Toolbar, "SELECT")
                        .get_by_label(entry.label)
                        .click();
                    step(&mut h);
                    step(&mut h);
                    assert_eq!(kinds(&h), (true, true, true));
                }
            }
            Command::Ok | Command::Cancel => {
                let f = fixture();
                let (mut h, _) = f.opened("example.silico", EXAMPLE);
                step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
                let ok = command == Command::Ok;
                let boxes = |h: &AppHarness| text_of(&f, h).matches("\"box\"").count();
                let mut kept = boxes(&h);
                for chord in keys_of(command) {
                    for by_keys in [true, false] {
                        create(&mut h, "Box");
                        added(&mut h);
                        settled(&mut h);
                        // Its preview built: Esc would stop one still rebuilding first.
                        step_until(&mut h, "the preview built", |h| {
                            let tab = &h.state().tabs()[0];
                            tab.status.as_ref().is_some_and(|status| {
                                status.rebuild.revision == tab.revision
                                    && status.rebuild.state
                                        == silico_core::command::RebuildState::Done
                            })
                        });
                        if by_keys {
                            press_chord(&mut h, chord);
                        } else {
                            dialog(&h)
                                .get_by_role_and_label(Role::Button, entry.label)
                                .click();
                        }
                        step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
                        settled(&mut h);
                        kept += usize::from(ok);
                        assert_eq!(boxes(&h), kept, "{command:?}, by its keys: {by_keys}");
                    }
                }
            }
            Command::Shortcuts => {
                let f = fixture();
                let (mut h, _) = f.opened("example.silico", EXAMPLE);
                step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
                for chord in keys_of(command) {
                    press_chord(&mut h, chord);
                    step_until(&mut h, "the sheet by its keys", sheet_open);
                    h.key_press(Key::Escape);
                    step_until(&mut h, "closed", |h| !sheet_open(h));
                    h.get_by_role_and_label(Role::Button, "Shortcuts").click();
                    step_until(&mut h, "the sheet by its button", sheet_open);
                    h.key_press(Key::Escape);
                    step_until(&mut h, "closed", |h| !sheet_open(h));
                }
            }
        }
    }
    // The tooltips name the keys the registry does: Save's, Undo's, and Redo's.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    for (button, caps) in [
        ("Save", ["Ctrl", "S"]),
        ("Undo", ["Ctrl", "Z"]),
        ("Redo", ["Ctrl", "Y"]),
    ] {
        h.get_by_role_and_label(Role::Button, button).hover();
        step_until(&mut h, "its tooltip", |h| {
            h.query_all_by_role_and_label(Role::Label, button).count() > 0
        });
        for _ in 0..3 {
            step(&mut h);
        }
        let texts: Vec<String> = h
            .output()
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                eframe::egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        for cap in caps {
            assert!(
                texts.iter().any(|text| text == cap),
                "{button}'s {cap}: {texts:?}"
            );
        }
        pointer_away(&mut h);
    }
    assert_eq!(never_waits::waits(), 0);
}

/// A block alone: its box is its body.
const ONE_BLOCK: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

#[test]
fn ctrl_shift_b_after_a_kept_fault_shows_the_model_it_built() {
    // The box's first build crashes its worker: a kernel fault, which the revision keeps as
    // its outcome (decision 32) and its model publishes, with no body.
    let f = fixture();
    let crashed = Arc::new(AtomicBool::new(false));
    let once = Arc::clone(&crashed);
    f.kernel.refuse(move |op| {
        (is_box(op) && !once.swap(true, Ordering::SeqCst))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
    });
    let (mut h, _record) = f.drawn("block.silico", ONE_BLOCK);
    step_until(&mut h, "faulted", |h| {
        h.state()
            .tabs()
            .first()
            .and_then(|tab| tab.status.as_ref())
            .is_some_and(|status| {
                status.rebuild.state == silico_core::command::RebuildState::Faulted
            })
    });
    meshes_settled(&mut h);
    assert!(crashed.load(Ordering::SeqCst));
    assert!(
        view(&h).bodies().is_empty(),
        "the fault's model has no body"
    );
    let revision = h.state().tabs()[0].published;
    // Ctrl+Shift+B tries the box again: a new model of the same revision, its block a
    // body, which the view reads.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::B);
    let block = silico_core::document::FeatureId::new("k7f2qx3m").unwrap();
    step_until(&mut h, "the block in the view", |h| {
        view(h).bodies().values().any(|body| body.feature == block)
    });
    assert_eq!(h.state().tabs()[0].published, revision, "the same revision");
    assert_eq!(never_waits::waits(), 0);
}

// ---- #382's mutants: a frame whose key acts asks for the next at once ----

/// The line of app.rs on which a frame that started something, by a key or a control, asks
/// for the next frame at once (`after_drawing`'s `request_repaint`), found in its source.
fn started_line() -> u32 {
    let source = include_str!("../src/app.rs");
    let from = source.find("fn after_drawing").expect("after_drawing");
    let at = from
        + source[from..]
            .find("ui.ctx().request_repaint();")
            .expect("its ask for the next frame");
    u32::try_from(source[..at].matches('\n').count() + 1).unwrap()
}

/// Whether the frame that takes `key`'s press, `modifiers` held, asks for the next at
/// once itself, beyond egui's own ask after any input (`asked_from_app`): the app idle
/// first, its idle frame asking no such thing. Each event its own frame, as the harness
/// steps them, one pass each.
fn asks_at_once_after(h: &mut AppHarness, modifiers: Modifiers, key: Key) -> bool {
    // One pass a frame: a second, which egui runs when a layout asks it to, would take the
    // causes the frame before asked for out of `repaint_causes`.
    h.ctx
        .options_mut(|options| options.max_passes = std::num::NonZeroUsize::MIN);
    quiet_wakes(h);
    idle(h);
    step(h);
    step(h);
    assert!(
        !asked_from_app(h).contains(&started_line()),
        "an idle frame asked"
    );
    let event = |pressed| eframe::egui::Event::Key {
        key,
        pressed,
        modifiers,
        repeat: false,
        physical_key: None,
    };
    h.event(eframe::egui::Event::ModifiersChanged(modifiers));
    step(h);
    // The key's frame; then the next, its release's, whose causes are the key frame's.
    h.event(event(true));
    step(h);
    h.event(event(false));
    step(h);
    let asked = asked_from_app(h).contains(&started_line());
    h.event(eframe::egui::Event::ModifiersChanged(Modifiers::NONE));
    step(h);
    asked
}

#[test]
fn a_key_that_is_no_command_asks_for_no_frame_itself() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    settled(&mut h);
    assert!(!asks_at_once_after(&mut h, Modifiers::NONE, Key::Q));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_s_alone_asks_for_the_next_frame_at_once() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "example.silico *")
    });
    assert!(asks_at_once_after(&mut h, Modifiers::COMMAND, Key::S));
    step_until(&mut h, "saved", |h| !anywhere(h, "example.silico *"));
    assert_eq!(names_in(&path), ["Block", "PinK", "Hole"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_z_alone_asks_for_the_next_frame_at_once() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    type_name(&mut h, "K");
    step_until(&mut h, "Undo offered", |h| {
        h.state().tabs()[0].edits.idle() && offered(h, "Undo")
    });
    assert!(asks_at_once_after(&mut h, Modifiers::COMMAND, Key::Z));
    step_until(&mut h, "undone", |h| shows(h, "Timeline", "Pin"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_y_alone_asks_for_the_next_frame_at_once() {
    let f = fixture();
    let (mut h, _) = undone_back(&f);
    assert!(asks_at_once_after(&mut h, Modifiers::COMMAND, Key::Y));
    step_until(&mut h, "redone", |h| shows(h, "Timeline", "Pinx"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_alone_asks_for_the_next_frame_at_once() {
    // A selection held: Esc clears it (section 3's order).
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Pin"));
    select(&mut h, "Pin");
    assert!(h.state().tabs()[0].selection().feature().is_some());
    assert!(asks_at_once_after(&mut h, Modifiers::NONE, Key::Escape));
    assert!(h.state().tabs()[0].selection().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_w_alone_asks_for_the_next_frame_at_once() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    settled(&mut h);
    assert!(asks_at_once_after(&mut h, Modifiers::COMMAND, Key::W));
    step_until(&mut h, "closed", |h| h.state().tabs().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn enter_alone_in_a_dialog_asks_for_the_next_frame_at_once() {
    // The dialog's OK, which its pane asks the next frame for too, so this alone never
    // tells `keys`' own `enter` from none (Ctrl+W's and Ctrl+B's tests do, beside it).
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    create(&mut h, "Box");
    added(&mut h);
    settled(&mut h);
    assert!(asks_at_once_after(&mut h, Modifiers::NONE, Key::Enter));
    step_until(&mut h, "the dialog's OK", |h| !dialog_open(h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_b_alone_asks_for_the_next_frame_at_once() {
    let f = fixture();
    let (mut h, hold) = first_rebuild_stopped(&f);
    hold.release();
    assert!(h.state().tabs()[0].can_rebuild());
    assert!(asks_at_once_after(&mut h, Modifiers::COMMAND, Key::B));
    assert_eq!(h.state().tabs()[0].rebuilds, 1);
    assert_eq!(h.state().tabs()[0].rebuilds_all, 0);
    step_until(&mut h, "the timeline", |h| shows(h, "Timeline", "Hole"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn ctrl_shift_b_alone_asks_for_the_next_frame_at_once() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| shows(h, "Timeline", "Hole"));
    settled(&mut h);
    assert!(asks_at_once_after(
        &mut h,
        Modifiers::COMMAND | Modifiers::SHIFT,
        Key::B
    ));
    assert_eq!(h.state().tabs()[0].rebuilds_all, 1);
    assert_eq!(h.state().tabs()[0].rebuilds, 0);
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
    // The dialogs off: the toolbar's SKETCH tab holds none of them (its CREATE is the
    // sketch's tools), and none can open.
    assert!(
        h.get_by_role_and_label(Role::Toolbar, "CREATE")
            .query_by_label("Pad")
            .is_none()
    );
    assert!(!h.state().tabs()[0].can_open(silico_app::dialog::Kind::Box));
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
    assert!(before.contains(r#""name": "PinK""#));
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
    assert_eq!(
        h.state().tabs()[0].selection().feature(),
        Some(&sketch_id("s0000001"))
    );
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(sketch_mode(&h).is_some());
    assert!(h.state().tabs()[0].selection().is_empty());
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

// ---- #351's mutants: sketch mode's panels placed, its idle frames, and its Delete ----

/// The prototype's `--g200` (floating panels), `--g250` (their border), and `--blue` (the
/// sketch bar's border).
const G200: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(0x21, 0x21, 0x21);
const G250: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(0x27, 0x26, 0x26);
const BLUE: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(0x18, 0x6a, 0xdd);

/// Triangle open in sketch mode, read, and everything answered.
fn triangle_in_sketch_mode(f: &Fixture) -> AppHarness {
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    settled(&mut h);
    h
}

/// The rectangles the last frame painted filled `--g200` and bordered in `border`, standing
/// in the canvas (from their top left: a palette taller than the canvas's room runs past its
/// bottom).
fn floating_frames(h: &AppHarness, border: eframe::egui::Color32) -> Vec<eframe::egui::Rect> {
    let canvas = h.get_by_role_and_label(Role::Pane, "Viewport").rect();
    painted(h)
        .into_iter()
        .filter_map(|shape| match shape {
            eframe::egui::Shape::Rect(rect)
                if rect.fill == G200
                    && rect.stroke.color == border
                    && canvas.expand(0.5).contains(rect.rect.left_top()) =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_sketch_bar_and_the_palette_float_over_the_canvas_where_the_prototype_floats_them() {
    // styles.css: `.sketch-bar` across the canvas's middle, 10 below its top (`left: 50%;
    // top: 10px; transform: translateX(-50%)`), bordered `--blue`; `.palette` 12 in from the
    // canvas's right and 118 down (`right: 12px; top: 118px`), 212 wide, so its left 224 in,
    // bordered `--g250`. Each `--g200`.
    let f = fixture();
    let mut h = triangle_in_sketch_mode(&f);
    // The bar is centred on its width, which its state's words set once read.
    for _ in 0..3 {
        step(&mut h);
    }
    let canvas = h.get_by_role_and_label(Role::Pane, "Viewport").rect();
    let bars = floating_frames(&h, BLUE);
    let [bar] = bars.as_slice() else {
        panic!("{bars:?}");
    };
    assert!(
        (bar.center().x - canvas.center().x).abs() <= 0.5,
        "{bar:?} in {canvas:?}"
    );
    assert_eq!(bar.top(), canvas.top() + 10.0, "{bar:?} in {canvas:?}");
    let palettes = floating_frames(&h, G250);
    let [palette] = palettes.as_slice() else {
        panic!("{palettes:?}");
    };
    assert_eq!(palette.left(), canvas.right() - 12.0 - 212.0, "{palette:?}");
    assert_eq!(palette.top(), canvas.top() + 118.0, "{palette:?}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn sketch_mode_left_alone_asks_for_no_frame_sooner_than_the_idle_one() {
    // Nothing pending and no input: each frame asks for the next at the idle interval (less
    // the quarter second a harness's frame takes, which egui takes off it), never at once
    // (the bar asked nothing).
    let f = fixture();
    let mut h = triangle_in_sketch_mode(&f);
    for _ in 0..10 {
        step(&mut h);
    }
    let frame = Duration::from_millis(250);
    for _ in 0..5 {
        step(&mut h);
        let delay = repaint_delay(&h);
        assert!(
            delay.is_some_and(|delay| delay >= IDLE_FRAME - frame && delay <= IDLE_FRAME),
            "{delay:?}"
        );
    }
    assert!(sketch_mode(&h).is_some());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn delete_and_backspace_in_one_frame_delete_the_selection_once() {
    let f = fixture();
    let mut h = triangle_in_sketch_mode(&f);
    select_member(&mut h, "na000000", silico_core::document::ElementKind::Line);
    // Both pressed in the one frame (the harness's own presses take a frame each), then
    // both let go in the next.
    use eframe::egui::{Event, Key, Modifiers};
    for pressed in [true, false] {
        for key in [Key::Delete, Key::Backspace] {
            h.input_mut().events.push(Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            });
        }
        step(&mut h);
    }
    settled(&mut h);
    let removes = h.state().tabs()[0]
        .sent
        .iter()
        .filter(|sent| **sent == Sent::SketchEdit("sketch_remove"))
        .count();
    assert_eq!(removes, 1);
    let text = text_of(&f, &h);
    assert!(!text.contains("na000000"), "{text}");
    assert!(
        text.contains("nb000000") && text.contains("nc000000"),
        "{text}"
    );
    assert_eq!(never_waits::waits(), 0);
}

/// One sketch, Loose, of one point.
const ONE_SKETCH: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Loose", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

#[test]
fn a_lone_sketchs_prefix_is_read_once_kept_and_drawn_before_its_name_in_the_browser() {
    use silico_protocol::Solved;
    // The fake solves the point free to move: under defined, `(–)`.
    let f = fixture();
    f.sketches.answer(|system| {
        Ok(Solved {
            dofs: 2,
            free: system.unknowns.clone(),
            ..FakeSketches::as_sent(system)
        })
    });
    let (mut h, _) = f.opened("one.silico", ONE_SKETCH);
    let loose = sketch_id("s0000001");
    step_until(&mut h, "its prefix", |h| {
        h.state().tabs()[0].prefixes.prefix(&loose) == Some("(–)")
    });
    // Read once for the model, and kept, however many frames pass.
    for _ in 0..20 {
        step(&mut h);
    }
    let tab = &h.state().tabs()[0];
    assert_eq!(tab.prefixes.reads, 1);
    assert_eq!(tab.prefixes.prefix(&loose), Some("(–)"));
    // Its browser row draws it (`.row .prefix.under`, `--sk-under` #7a95ff) before its name.
    let row = h
        .get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Loose")
        .rect();
    let texts = texts_in(&h, row);
    assert!(
        texts.contains(&(
            "(–)".to_owned(),
            eframe::egui::Color32::from_rgb(0x7a, 0x95, 0xff)
        )),
        "{texts:?}"
    );
    let left = |words: &str| {
        painted_in(&h, row)
            .into_iter()
            .find_map(|shape| match shape {
                eframe::egui::Shape::Text(text) if text.galley.text() == words => Some(text.pos.x),
                _ => None,
            })
    };
    let (prefix, name) = (left("(–)").unwrap(), left("Loose").unwrap());
    assert!(prefix < name, "{prefix} and {name}");
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

#[test]
fn a_sketchs_error_card_edit_opens_sketch_mode_as_its_double_click_does() {
    // The face sketch's broken plane is its error: its card's Edit is Edit Sketch, not
    // Edit Feature (which no dialog of a sketch's answers).
    let f = fixture();
    let (mut h, path) = f.opened("face.silico", ON_A_FACE);
    step_until(&mut h, "its error", |h| {
        h.query_by_role_and_label(Role::Button, "Rebuilt with 1 error")
            .is_some()
    });
    assert!(sketch_mode(&h).is_none());
    h.get_by_role_and_label(Role::Button, "Rebuilt with 1 error")
        .click();
    // Block's card first, its file storing no fingerprint (a warning), then Top's.
    step_until(&mut h, "its card listed", |h| {
        in_whats_wrong(h, "Feature: ") == ["Feature: Block b0000001", "Feature: Top s0000001"]
    });
    step(&mut h);
    // Offered: sketch mode may open (no dialog, no sketch, the gate taking the document).
    let edit_off = |h: &AppHarness| {
        whats_wrong(h)
            .unwrap()
            .get_by_role_and_label(Role::Button, "Edit Top")
            .accesskit_node()
            .is_disabled()
    };
    assert!(!edit_off(&h), "Edit Top offered");
    whats_wrong(&h)
        .unwrap()
        .get_by_role_and_label(Role::Button, "Edit Top")
        .click();
    step_until(&mut h, "sketch mode on Top", |h| {
        whats_wrong(h).is_none()
            && sketch_mode(h).is_some_and(|mode| mode.sketch == Some(sketch_id("s0000001")))
    });
    assert!(!dialog_open(&h));
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
        mode.palette_view(h.state().tabs()[0].revision).said
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

/// SKETCHED open, with Plate renamed PlateK first if `edited`; then Triangle opened in
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
    assert!(before.contains(r#""name": "PlateK""#));
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
    assert!(text_of(&f, &h).contains(r#""name": "PlateK""#));
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
    select(&mut h, "PlateK");
    type_name(&mut h, "Q");
    settled(&mut h);
    chord(&mut h, eframe::egui::Key::Z);
    assert!(h.state().tabs()[0].redoable());
    chord(&mut h, eframe::egui::Key::Y);
    assert!(text_of(&f, &h).contains(r#""name": "PlateKQ""#));
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

/// The projection the tools are driven through until the pane's overlay gives its own
/// (#351): ten points a millimetre, the origin at (100, 500), y up.
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
/// gives them (`Tab::sketch_frame`, which the pane calls once #351 wires it), on a thread
/// marked so a wait there counts, the pointer at the last; then one frame of the app's.
fn sketch_events(f: &Fixture, h: &mut AppHarness, events: &[silico_app::sketch::PointerEvent]) {
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
            mode.frame(&ui.painter().clone(), &input);
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

/// The crossed sketch, and a box beside it, open in sketch mode, its na000001 trimmed (an
/// edit, so a close asks), and its tab closing: Close's Save chosen, the save held behind a
/// fresh check of the box's rebuild. The hold, and the check, to let go of and wait on.
fn closing_in_sketch(f: &Fixture) -> (AppHarness, Hold, Running<CheckAnswer>) {
    let block = r#"    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
    let text = CROSSED.replace("\n  ]\n}", &format!(",\n{block}\n  ]\n}}"));
    let (mut h, _) = f.opened("crossed.silico", &text);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Cross"));
    double_click(&mut h, "Browser", "Cross");
    sketch_read(&mut h);
    h.key_press(eframe::egui::Key::T);
    step(&mut h);
    // A click on na000001 at (30, 0.3) trims it: an edit, so the close asks.
    sketch_events(f, &mut h, &clicked(30.0, 0.3));
    settled(&mut h);
    read_with_lines(&mut h, 2);
    // Closing, its save held.
    let (hold, check) = hold_queue(f, &h, 0);
    h.get_by_label("Close crossed.silico").click();
    step_until(&mut h, "the question", |h| {
        anywhere(h, "Save changes to crossed.silico before closing?")
    });
    h.query_all_by_role_and_label(Role::Button, "Save")
        .last()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "the save sent", |h| h.state().tabs()[0].saves == 1);
    step(&mut h);
    assert!(h.state().tabs()[0].closing());
    (h, hold, check)
}

#[test]
fn a_trim_asked_while_the_tab_closes_is_never_queued() {
    let f = fixture();
    let (mut h, hold, check) = closing_in_sketch(&f);
    // What the tab says, which an edit queued would let go of.
    h.state_mut().tabs_mut()[0].message = Some("kept".to_owned());
    let sent = h.state().tabs()[0].sent.clone();
    // A click on na000002 at (20.3, 5): its trim asked, but the closing tab's queue takes
    // no edit, so none is queued and what it says stays.
    sketch_events(&f, &mut h, &clicked(20.3, 5.0));
    assert_eq!(h.state().tabs()[0].message.as_deref(), Some("kept"));
    assert_eq!(h.state().tabs()[0].sent, sent);
    hold.release();
    check.wait();
    assert_eq!(never_waits::waits(), 0);
}

// ---- #364's mutants, the second round: sketch mode's keys, its tool chosen and its chain
// ended from outside the palette, and what its tool asks ----

/// The sketched part's Triangle open in sketch mode, and read.
fn triangle_sketch(f: &Fixture) -> AppHarness {
    let (mut h, _) = f.opened("sketched.silico", SKETCHED);
    step_until(&mut h, "its sketch", |h| shows(h, "Browser", "Triangle"));
    double_click(&mut h, "Browser", "Triangle");
    sketch_read(&mut h);
    h
}

#[test]
fn a_segment_drawn_while_the_tab_closes_is_refused_to_its_tool_and_never_queued() {
    use silico_core::sketch_tools::ToolKind;
    let f = fixture();
    let (mut h, hold, check) = closing_in_sketch(&f);
    h.state_mut().tabs_mut()[0].message = Some("kept".to_owned());
    let sent = h.state().tabs()[0].sent.clone();
    h.state_mut().tabs_mut()[0].sketch_tool(Some(ToolKind::Line));
    // A segment from (5, 20) to (15, 25), clear of both lines: its add asked, but the
    // closing tab's queue takes no edit, so it's refused to the tool, none is queued, and
    // what the tab says stays.
    sketch_events(&f, &mut h, &clicked(5.0, 20.0));
    sketch_events(&f, &mut h, &clicked(15.0, 25.0));
    assert_eq!(h.state().tabs()[0].message.as_deref(), Some("kept"));
    assert_eq!(h.state().tabs()[0].sent, sent);
    assert_eq!(adds_sent(&h), 0);
    hold.release();
    check.wait();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn enter_alone_in_sketch_mode_ends_the_chain_and_asks_for_the_next_frame_at_once() {
    use silico_app::sketch::tools::Choice;
    use silico_core::sketch_tools::ToolKind;
    let f = fixture();
    let mut h = triangle_sketch(&f);
    h.key_press(Key::L);
    step(&mut h);
    // A chain's first click, at (60, 30), clear of the triangle: under way, nothing added.
    sketch_events(&f, &mut h, &clicked(60.0, 30.0));
    assert!(sketch_mode(&h).unwrap().tools.busy());
    // Enter alone ends the chain, the tool kept (section 9, *Line*): a command, so its
    // frame asks for the next at once.
    assert!(asks_at_once_after(&mut h, Modifiers::NONE, Key::Enter));
    let mode = sketch_mode(&h).unwrap();
    assert!(!mode.tools.busy());
    assert_eq!(mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn command_enter_alone_in_sketch_mode_finishes_it_and_asks_for_the_next_frame_at_once() {
    let f = fixture();
    let mut h = triangle_sketch(&f);
    settled(&mut h);
    // ⌘↵ alone, the prototype's Finish Sketch: sketch mode closed, and its frame asks for
    // the next at once.
    assert!(asks_at_once_after(&mut h, Modifiers::COMMAND, Key::Enter));
    assert!(sketch_mode(&h).is_none());
    settled(&mut h);
    assert_eq!(text_of(&f, &h), SKETCHED);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn sketch_tool_chooses_a_drawing_tool_that_then_draws_and_none_leaves_it() {
    use silico_app::sketch::tools::Choice;
    use silico_core::sketch_tools::ToolKind;
    let f = fixture();
    let mut h = triangle_sketch(&f);
    let tool = |h: &AppHarness| sketch_mode(h).unwrap().tools.choice();
    h.state_mut().tabs_mut()[0].sketch_tool(Some(ToolKind::CornerRectangle));
    assert_eq!(tool(&h), Some(Choice::Draw(ToolKind::CornerRectangle)));
    h.state_mut().tabs_mut()[0].sketch_tool(Some(ToolKind::Line));
    assert_eq!(tool(&h), Some(Choice::Draw(ToolKind::Line)));
    // It draws: a segment from (30, 30) to (40, 35), one add, its two points new.
    sketch_events(&f, &mut h, &clicked(30.0, 30.0));
    sketch_events(&f, &mut h, &clicked(40.0, 35.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 1);
    let sketch = sketch_of_text(&f, &h, "s0000001");
    let (a, b) = (point_at(&sketch, 30.0, 30.0), point_at(&sketch, 40.0, 35.0));
    assert!(line_between(&sketch, &a, &b).is_some());
    // None: the tool left.
    h.state_mut().tabs_mut()[0].sketch_tool(None);
    assert_eq!(tool(&h), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn sketch_end_chain_ends_the_chain_under_way_and_keeps_the_tool() {
    use silico_app::sketch::tools::Choice;
    use silico_core::sketch_tools::ToolKind;
    let f = fixture();
    let mut h = triangle_sketch(&f);
    h.key_press(Key::L);
    step(&mut h);
    sketch_events(&f, &mut h, &clicked(60.0, 30.0));
    assert!(sketch_mode(&h).unwrap().tools.busy());
    // The canvas menu's End chain, as Enter: the chain ended, the tool kept.
    {
        let sessions = Arc::clone(&f.sessions);
        let _marked = never_waits::marked();
        h.state_mut().tabs_mut()[0].sketch_end_chain(&sessions);
    }
    let mode = sketch_mode(&h).unwrap();
    assert!(!mode.tools.busy());
    assert_eq!(mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    // So the next click begins a chain, and nothing is added.
    sketch_events(&f, &mut h, &clicked(70.0, 30.0));
    settled(&mut h);
    assert_eq!(adds_sent(&h), 0);
    assert_eq!(text_of(&f, &h), SKETCHED);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tools_add_lets_go_of_what_was_said_and_of_what_sketch_mode_undid() {
    let f = fixture();
    let mut h = triangle_sketch(&f);
    h.key_press(Key::L);
    step(&mut h);
    // A segment from (30, 30) to (40, 35), its chain ended by Enter; then undone by Ctrl+Z,
    // one of sketch mode's own steps undone.
    sketch_events(&f, &mut h, &clicked(30.0, 30.0));
    sketch_events(&f, &mut h, &clicked(40.0, 35.0));
    settled(&mut h);
    read_with_lines(&mut h, 4);
    h.key_press(Key::Enter);
    step(&mut h);
    step(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    step(&mut h);
    settled(&mut h);
    read_with_lines(&mut h, 3);
    assert_eq!(sketch_mode(&h).unwrap().undone, 1);
    assert_eq!(text_of(&f, &h), SKETCHED);
    // Something said meanwhile; then a segment from (50, 30) to (60, 35), its one add a new
    // edit: what was said let go, and what sketch mode undid dropped (section 9, as
    // Delete's edit drops it).
    h.state_mut().tabs_mut()[0].message = Some("said before".to_owned());
    sketch_events(&f, &mut h, &clicked(50.0, 30.0));
    sketch_events(&f, &mut h, &clicked(60.0, 35.0));
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(sketch_mode(&h).unwrap().undone, 0);
    settled(&mut h);
    assert_eq!(adds_sent(&h), 2);
    assert_eq!(never_waits::waits(), 0);
}

// ---- Untitled parts (docs/design/m5.0-app.md §4, New; open item 30, overturned) ----
//
// New opens a part with no file (`open_untitled`), named "Untitled", "Untitled 2", ...; its
// first save is a Save As through the Save panel (the stand-in, as above), and with no
// native dialogs, or one that won't open, through the typed Save As. Expected names and
// paths are written here by hand; files' contents are the session's text, read from the
// core, and the features' names in them.

/// Command+N pressed, and the new part's tab waited for.
fn new_part(h: &mut AppHarness) {
    let before = h.state().tabs().len();
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::N);
    step_until(h, "the untitled part's tab", |h| {
        h.state().tabs().len() == before + 1
    });
}

/// The tab named `name`.
fn tab_named<'a>(h: &'a AppHarness, name: &str) -> &'a silico_app::tab::Tab {
    h.state()
        .tabs()
        .iter()
        .find(|tab| tab.name() == name)
        .unwrap_or_else(|| panic!("no tab {name}"))
}

/// A box added to the part shown, named `name`, through CREATE's Box dialog and its OK:
/// the part then holds unsaved edits, its tab marked.
fn add_box(h: &mut AppHarness, name: &str) {
    create(h, "Box");
    step_until(h, "its add answered", |h| {
        tab_named(h, name)
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    dialog(h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(h, "the dialog closed", |h| !dialog_open(h));
    let marked = format!("{name} *");
    step_until(h, "unsaved", |h| {
        tab_named(h, name).edits.idle() && tab_named(h, name).unsaved() && anywhere(h, &marked)
    });
}

/// The question's button `label`, the last of that label (after the app bar's Save).
fn answer_question(h: &mut AppHarness, label: &str) {
    h.query_all_by_role_and_label(Role::Button, label)
        .last()
        .unwrap()
        .click_accesskit();
    step(h);
}

#[test]
fn new_names_parts_untitled_then_untitled_2_and_never_gives_a_name_again() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    new_part(&mut h);
    new_part(&mut h);
    let names = |h: &AppHarness| -> Vec<String> {
        h.state()
            .tabs()
            .iter()
            .map(|tab| tab.name().to_owned())
            .collect()
    };
    assert_eq!(names(&h), ["Untitled", "Untitled 2"]);
    // Untitled 2, holding no edits, closed without asking; then New: Untitled 3, the name
    // Untitled 2 had never given again in this run.
    h.get_by_label("Close Untitled 2").click();
    step_until(&mut h, "it closed", |h| h.state().tabs().len() == 1);
    assert!(!anywhere(&h, "Save changes to Untitled 2 before closing?"));
    new_part(&mut h);
    assert_eq!(names(&h), ["Untitled", "Untitled 3"]);
    step_until(&mut h, "Untitled 3 shown", |h| {
        toggled(&h.get_by_role_and_label(Role::Button, "Untitled 3")) == Some(true)
    });
    // Each a session of its own, none with a file.
    let tabs = h.state().tabs();
    assert_ne!(tabs[0].id, tabs[1].id);
    for tab in tabs {
        assert_eq!(tab.path, None);
        assert_eq!(
            f.sessions.status(&tab.id).session.unwrap().path,
            None::<String>
        );
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_untitled_parts_edit_is_marked_and_command_s_saves_it_through_the_save_panel() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    assert!(anywhere(&h, "Untitled *"));
    // Command+S: the Save panel, named by the part; nothing written until it answers.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    assert_eq!(
        browser.asked(),
        [Ask {
            pick: Pick::Save,
            title: "Save Untitled as".to_owned(),
            filters: vec![browse_mod::DOCUMENTS],
            folder: None,
            file_name: Some("Untitled.silico".to_owned()),
        }]
    );
    // Command+S again while it's open: no second panel, nothing sent.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(h.state().tabs()[0].saves, 0);
    assert!(!h.state().tabs()[0].sent.contains(&Sent::Commit));
    // Picked: committed to it, the tab renamed and clean.
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "renamed and clean", |h| {
        anywhere(h, "part.silico") && !anywhere(h, "part.silico *")
    });
    let tab = &h.state().tabs()[0];
    assert_eq!(tab.path.as_deref(), part.to_str());
    assert_eq!(tab.name(), "part.silico");
    assert_eq!(tab.saves, 1);
    assert!(!tab.unsaved());
    assert_eq!(tab.message, None);
    assert!(!anywhere(&h, "Untitled"));
    // The file holds the document as the session holds it: the box.
    let id = tab.id;
    let held = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert_eq!(std::fs::read_to_string(&part).unwrap(), held);
    assert_eq!(names_in(&part), ["Box"]);
    assert_eq!(
        f.sessions.status(&id).session.unwrap().path.as_deref(),
        part.to_str()
    );
    // From now on a file's: Command+S commits to it, asking nothing.
    select(&mut h, "Box");
    type_name(&mut h, "x");
    step_until(&mut h, "unsaved", |h| {
        h.state().tabs()[0].edits.idle() && anywhere(h, "part.silico *")
    });
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    step_until(&mut h, "saved", |h| !anywhere(h, "part.silico *"));
    assert_eq!(names_in(&part), ["Boxx"]);
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn closing_an_edited_untitled_part_asks_and_dont_save_leaves_nothing_and_a_cancel_keeps_it() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    let before = listing(f.dir.path());
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    let question = "Save changes to Untitled before closing?";
    // Close: asked. Save: the Save panel, cancelled: the part stays, its edits held, and
    // nothing is said or sent.
    h.get_by_label("Close Untitled").click();
    step_until(&mut h, "the question", |h| anywhere(h, question));
    answer_question(&mut h, "Save");
    save_panel_asked(&mut h, &browser, 1);
    assert_eq!(
        browser.asked()[0].file_name.as_deref(),
        Some("Untitled.silico")
    );
    browser.answer(None);
    for _ in 0..10 {
        step(&mut h);
    }
    let tab = &h.state().tabs()[0];
    assert_eq!(h.state().tabs().len(), 1);
    assert!(tab.unsaved() && tab.transaction && !tab.closing());
    assert_eq!(tab.saves, 0);
    assert_eq!(h.state().message(), None);
    assert!(!anywhere(&h, question));
    // Close again, Don't save: closed, nothing left on disk, no panel asked.
    h.get_by_label("Close Untitled").click();
    step_until(&mut h, "the question", |h| anywhere(h, question));
    answer_question(&mut h, "Don't save");
    step_until(&mut h, "it closed", |h| h.state().tabs().is_empty());
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(listing(f.dir.path()), before);
    // A quit's Save all, its panel cancelled: the quit stops, the part kept, the window
    // open, and that said.
    new_part(&mut h);
    add_box(&mut h, "Untitled 2");
    close_window(&mut h);
    assert!(told(&h, false), "a close with edits held wasn't cancelled");
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    save_panel_asked(&mut h, &browser, 2);
    assert_eq!(browser.asked()[1].title, "Save Untitled 2 as");
    browser.answer(None);
    step_until(&mut h, "the quit stopped", |h| {
        h.state().message() == Some("Quit stopped: Untitled 2 wasn't closed")
    });
    for _ in 0..20 {
        step(&mut h);
        assert!(!told(&h, true), "the window closed with Untitled 2's edits");
    }
    let tab = &h.state().tabs()[0];
    assert_eq!(h.state().tabs().len(), 1);
    assert!(tab.unsaved() && !tab.closing());
    assert_eq!(browser.asked().len(), 2);
    assert_eq!(listing(f.dir.path()), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn quitting_with_two_edited_untitled_parts_saves_each_through_its_panel_in_turn() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    new_part(&mut h);
    add_box(&mut h, "Untitled 2");
    close_window(&mut h);
    assert!(told(&h, false), "a close with edits held wasn't cancelled");
    step_until(&mut h, "the quit question", |h| {
        anywhere(h, "Save changes before quitting?")
            && h.query_by_label_contains("Unsaved: Untitled, Untitled 2. Quitting")
                .is_some()
    });
    answer_question(&mut h, "Save all");
    // The first's panel; the second's waits for it.
    save_panel_asked(&mut h, &browser, 1);
    assert_eq!(browser.asked()[0].title, "Save Untitled as");
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    let one = f.dir.path().join("one.silico");
    browser.answer(Some(&one));
    save_panel_asked(&mut h, &browser, 2);
    assert_eq!(browser.asked()[1].title, "Save Untitled 2 as");
    assert_eq!(
        browser.asked()[1].file_name.as_deref(),
        Some("Untitled 2.silico")
    );
    let two = f.dir.path().join("two.silico");
    browser.answer(Some(&two));
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    for path in [&one, &two] {
        assert_eq!(names_in(path), ["Box"], "{}", path.display());
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn quitting_with_three_untitled_parts_stops_at_the_one_whose_panel_is_cancelled() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    for name in ["Untitled", "Untitled 2", "Untitled 3"] {
        new_part(&mut h);
        add_box(&mut h, name);
    }
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    save_panel_asked(&mut h, &browser, 1);
    let one = f.dir.path().join("one.silico");
    browser.answer(Some(&one));
    save_panel_asked(&mut h, &browser, 2);
    assert_eq!(browser.asked()[1].title, "Save Untitled 2 as");
    browser.answer(None);
    step_until(&mut h, "the quit stopped and the first closed", |h| {
        h.state().message() == Some("Quit stopped: Untitled 2 wasn't closed")
            && h.state().tabs().len() == 2
    });
    // The third's panel never asked: the quit stopped before it.
    for _ in 0..20 {
        step(&mut h);
        assert!(!told(&h, true));
    }
    assert_eq!(browser.asked().len(), 2);
    let names: Vec<&str> = h.state().tabs().iter().map(|tab| tab.name()).collect();
    assert_eq!(names, ["Untitled 2", "Untitled 3"]);
    assert!(h.state().tabs().iter().all(|tab| tab.unsaved()));
    assert_eq!(names_in(&one), ["Box"]);
    // A second quit asks again, and its Save all asks the second's panel first.
    close_window(&mut h);
    step_until(&mut h, "asked again", |h| {
        anywhere(h, "Save changes before quitting?")
            && h.query_by_label_contains("Unsaved: Untitled 2, Untitled 3. Quitting")
                .is_some()
    });
    answer_question(&mut h, "Save all");
    save_panel_asked(&mut h, &browser, 3);
    assert_eq!(browser.asked()[2].title, "Save Untitled 2 as");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_untitled_parts_export_refuses_a_path_that_isnt_full_and_takes_one_that_is() {
    use silico_core::command::export::{Export, ExportFormat};
    let f = fixture();
    let documents = f.dir.path().join("Documents");
    let mut h = f.app_made(|app| app.documents_folder(Some(documents.clone())));
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Export…").click();
    step_until(&mut h, "the export dialog", |h| {
        anywhere(h, "Export Untitled")
    });
    // Its path starts in the Documents folder, named by the part.
    assert_eq!(
        path_value(&h),
        documents.join("Untitled.3mf").to_str().unwrap()
    );
    let refusal = "Give a full path: an untitled part has no folder";
    assert!(!anywhere(&h, refusal));
    press(&mut h, "STEP");
    for typed in ["out.step", "sub/out.step"] {
        set_field(&mut h, "Path", typed);
        step(&mut h);
        assert!(anywhere(&h, refusal), "{typed}");
        assert!(
            h.get_by_role_and_label(Role::Button, "Export")
                .accesskit_node()
                .is_disabled(),
            "{typed}"
        );
        press(&mut h, "Export");
        h.key_press(eframe::egui::Key::Enter);
        for _ in 0..5 {
            step(&mut h);
        }
        assert!(h.state().tabs()[0].exports.is_empty(), "{typed}");
        assert!(!f.dir.path().join(typed).exists(), "{typed}");
    }
    // A full path: taken, and written.
    let out = f.dir.path().join("out.step");
    set_field(&mut h, "Path", out.to_str().unwrap());
    step(&mut h);
    assert!(!anywhere(&h, refusal));
    press(&mut h, "Export");
    step_until(&mut h, "the dialog closed", |h| {
        !anywhere(h, "Export Untitled")
    });
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
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn with_no_native_dialog_an_untitled_parts_save_asks_for_a_typed_full_path() {
    let f = fixture();
    // No native dialogs at all, then a panel that won't open: the typed Save As each time.
    for refusing in [None, Some(FakeBrowser::default())] {
        let mut h = match &refusing {
            None => f.app(),
            Some(browser) => {
                browser.refusing(true);
                f.app_browsing(browser)
            }
        };
        step(&mut h);
        new_part(&mut h);
        add_box(&mut h, "Untitled");
        h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
        step_until(&mut h, "the typed Save As", |h| {
            anywhere(h, "Save Untitled as")
        });
        if let Some(browser) = &refusing {
            // The panel was asked for first.
            assert_eq!(browser.asked().len(), 1);
            assert_eq!(browser.asked()[0].pick, Pick::Save);
        }
        // A path that isn't full: refused, said, nothing sent.
        path_field(&h).focus();
        step(&mut h);
        path_field(&h).type_text("rel.silico");
        step(&mut h);
        step(&mut h);
        let refusal = "Give a full path: an untitled part has no folder";
        assert!(anywhere(&h, refusal));
        h.key_press(eframe::egui::Key::Enter);
        for _ in 0..5 {
            step(&mut h);
        }
        assert!(anywhere(&h, "Save Untitled as"));
        assert_eq!(h.state().tabs()[0].saves, 0);
        assert!(!Path::new("rel.silico").exists());
        // A full one: saved to it.
        let made = f
            .dir
            .path()
            .join(format!("typed-{}.silico", refusing.is_some()));
        set_field(&mut h, "Path", made.to_str().unwrap());
        step(&mut h);
        assert!(!anywhere(&h, refusal));
        answer_question(&mut h, "Save");
        step_until(&mut h, "saved as it", |h| {
            h.state().tabs()[0].path.as_deref() == made.to_str() && !anywhere(h, "Save Untitled as")
        });
        step_until(&mut h, "clean", |h| !h.state().tabs()[0].unsaved());
        assert_eq!(names_in(&made), ["Box"]);
        assert_eq!(never_waits::waits(), 0);
    }
}

#[test]
fn an_untitled_part_is_listed_in_recent_only_once_its_first_save_gives_it_a_file() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let file = f.recents_file();
    let mut h = f.app_made(|app| {
        app.remembering(Recents::at(file))
            .browsing(Box::new(browser.clone()))
    });
    recents_read(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    // Home, a while: nothing listed.
    home_button(&h).click();
    step_until(&mut h, "Home", start_shown);
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(recent_names(&h).is_empty());
    assert!(
        h.state()
            .recents()
            .is_some_and(|recents| recents.list().is_empty())
    );
    // Back to it, and saved: listed.
    h.key_press(eframe::egui::Key::Escape);
    step_until(&mut h, "the part", |h| !start_shown(h));
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "saved", |h| {
        h.state().tabs()[0].path.as_deref() == part.to_str()
    });
    home_button(&h).click();
    step_until(&mut h, "listed", |h| {
        start_shown(h) && recent_names(h) == ["part.silico"]
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn copy_mcp_config_with_an_untitled_part_shown_serves_the_newest_recent_ones_folder_then_documents()
{
    let f = fixture();
    let models = f.dir.path().join("models");
    std::fs::create_dir(&models).unwrap();
    let kept = models.join("b.silico");
    std::fs::write(&kept, EXAMPLE).unwrap();
    std::fs::create_dir_all(f.recents_file().parent().unwrap()).unwrap();
    std::fs::write(
        f.recents_file(),
        format!("silico recent-documents 1\n{}\n", kept.to_str().unwrap()),
    )
    .unwrap();
    let documents = f.dir.path().join("Documents");
    let file = f.recents_file();
    let mut h = f.app_made(|app| {
        app.serving_with("/opt/silico/bin/silico".to_owned())
            .documents_folder(Some(documents.clone()))
            .remembering(Recents::at(file))
    });
    recents_read(&mut h);
    new_part(&mut h);
    // The untitled part shown has no folder: the newest recent's is named.
    assert_eq!(
        h.state().mcp_config(),
        config_serving(models.to_str().unwrap())
    );
    home_button(&h).click();
    step_until(&mut h, "Home", start_shown);
    assert_eq!(
        copy_config(&mut h),
        Some(config_serving(models.to_str().unwrap()))
    );
    assert!(!anywhere(&h, silico_app::start::NO_WORKSPACE_NOTE));
    // With no recent list: the Documents folder.
    let mut bare = f.app_made(|app| {
        app.serving_with("/opt/silico/bin/silico".to_owned())
            .documents_folder(Some(documents.clone()))
    });
    step(&mut bare);
    new_part(&mut bare);
    assert_eq!(
        bare.state().mcp_config(),
        config_serving(documents.to_str().unwrap())
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn quits_save_all_with_a_parts_own_save_panel_open_saves_it_once_through_that_panel() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    let before = listing(f.dir.path());
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    // Command+S: its panel open (the portal's leaves the window usable), then a quit.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    for _ in 0..10 {
        step(&mut h);
    }
    // No second panel: the one open saves it, and closes it.
    assert_eq!(browser.asked().len(), 1);
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(names_in(&part), ["Box"]);
    // One document written: the part's, and no other.
    let documents = |listed: Vec<String>| -> Vec<String> {
        listed
            .into_iter()
            .filter(|name| name.ends_with(".silico"))
            .collect()
    };
    assert!(documents(before).is_empty());
    assert_eq!(documents(listing(f.dir.path())), ["part.silico"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn quits_save_all_while_a_parts_save_as_runs_waits_for_it_and_asks_no_panel() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    // Picked, its Save As held behind the session's queue.
    let (hold, check) = hold_queue(&f, &h, 0);
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "the Save As sent", |h| {
        h.state().tabs()[0].saving_as()
    });
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    assert!(!told(&h, true));
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(names_in(&part), ["Box"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_stopped_quit_leaves_a_save_panel_it_marked_to_save_without_closing() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    // A document whose save will be a conflict, and an untitled part with its panel open.
    let theirs_path = f.file("a.silico", EXAMPLE);
    h.state_mut().open(theirs_path.to_str().unwrap(), false);
    step_until(&mut h, "its features", |h| {
        h.query_by_role_and_label(Role::Pane, "Timeline").is_some() && shows(h, "Timeline", "Pin")
    });
    select(&mut h, "Pin");
    type_name(&mut h, "Q");
    step_until(&mut h, "unsaved", |h| anywhere(h, "a.silico *"));
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    let theirs = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Theirs""#);
    std::fs::write(&theirs_path, &theirs).unwrap();
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    step_until(&mut h, "the quit stopped", |h| {
        h.state().message() == Some("Quit stopped: a.silico wasn't closed")
    });
    // The pick now saves the part, and leaves it open.
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "saved", |h| {
        h.state()
            .tabs()
            .iter()
            .any(|tab| tab.path.as_deref() == part.to_str() && !tab.unsaved())
    });
    for _ in 0..20 {
        step(&mut h);
        assert!(!told(&h, true));
    }
    assert_eq!(h.state().tabs().len(), 2);
    assert!(h.state().tabs().iter().all(|tab| !tab.closing()));
    assert_eq!(names_in(&part), ["Box"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn with_no_native_dialogs_close_and_quit_save_an_untitled_part_through_the_typed_save_as() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    new_part(&mut h);
    add_box(&mut h, "Untitled 2");
    // Close's Save: the typed Save As, a full path, saved and closed.
    h.get_by_label("Untitled *").click();
    step(&mut h);
    h.get_by_label("Close Untitled").click();
    step_until(&mut h, "the close question", |h| {
        anywhere(h, "Save changes to Untitled before closing?")
    });
    answer_question(&mut h, "Save");
    step_until(&mut h, "the typed Save As", |h| {
        anywhere(h, "Save Untitled as")
    });
    let one = f.dir.path().join("one.silico");
    type_path(&mut h, &one);
    answer_question(&mut h, "Save");
    step_until(&mut h, "saved and closed", |h| {
        h.state().tabs().len() == 1 && h.state().tabs()[0].name() == "Untitled 2"
    });
    assert_eq!(names_in(&one), ["Box"]);
    // Quit's Save all, its typed Save As cancelled: the quit stops, the part kept.
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    step_until(&mut h, "the typed Save As", |h| {
        anywhere(h, "Save Untitled 2 as")
    });
    answer_question(&mut h, "Cancel");
    step_until(&mut h, "the quit stopped", |h| {
        h.state().message() == Some("Quit stopped: Untitled 2 wasn't closed")
    });
    for _ in 0..20 {
        step(&mut h);
        assert!(!told(&h, true));
    }
    assert!(h.state().tabs()[0].unsaved());
    // Quit again, Save all, a path to a file there, and Replace: saved over it and closed,
    // and the window with it.
    let there = f.file("there.silico", "an older copy");
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        h.query_by_label_contains("Save changes before quitting?")
            .is_some()
    });
    answer_question(&mut h, "Save all");
    step_until(&mut h, "the typed Save As", |h| {
        anywhere(h, "Save Untitled 2 as")
    });
    type_path(&mut h, &there);
    answer_question(&mut h, "Save");
    let asked = format!("{} already exists. Replace it?", there.to_str().unwrap());
    step_until(&mut h, "the Replace question", |h| anywhere(h, &asked));
    answer_question(&mut h, "Replace");
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    assert_eq!(names_in(&there), ["Box"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_untitled_parts_save_is_on_with_no_edits_and_asks_the_panel() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    step(&mut h);
    assert!(!h.state().tabs()[0].unsaved());
    let save = || {
        h.query_all_by_role_and_label(Role::Button, "Save")
            .next()
            .unwrap()
            .accesskit_node()
            .is_disabled()
    };
    assert!(!save(), "Save off on an untitled part");
    // The app bar's Save: the panel; cancelled, then Command+S: the panel again.
    h.query_all_by_role_and_label(Role::Button, "Save")
        .next()
        .unwrap()
        .click_accesskit();
    save_panel_asked(&mut h, &browser, 1);
    browser.answer(None);
    for _ in 0..5 {
        step(&mut h);
    }
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 2);
    let part = f.dir.path().join("empty.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "saved", |h| {
        h.state().tabs()[0].path.as_deref() == part.to_str()
    });
    // The empty document, as the session holds it.
    let id = h.state().tabs()[0].id;
    let held = f.sessions.text(&id, None).wait().result.unwrap().text;
    assert_eq!(std::fs::read_to_string(&part).unwrap(), held);
    assert!(names_in(&part).is_empty());
    // A file's now, with no edits: Save off.
    step_until(&mut h, "Save off", |h| {
        h.query_all_by_role_and_label(Role::Button, "Save")
            .next()
            .unwrap()
            .accesskit_node()
            .is_disabled()
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_untitled_parts_tab_and_save_say_it_has_no_file_when_hovered() {
    let f = fixture();
    let mut h = f.app();
    step(&mut h);
    new_part(&mut h);
    h.get_by_role_and_label(Role::Button, "Untitled").hover();
    step_until(&mut h, "the tab's tooltip", |h| {
        anywhere(h, "Not saved to a file yet")
    });
    h.query_all_by_role_and_label(Role::Button, "Save")
        .next()
        .unwrap()
        .hover();
    step_until(&mut h, "Save's tooltip", |h| {
        anywhere(h, "Asks where to save Untitled, then writes it")
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_untitled_parts_export_browse_starts_in_its_paths_folder_or_the_platforms() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let documents = f.dir.path().join("Documents");
    let mut h = f.app_made(|app| {
        app.documents_folder(Some(documents.clone()))
            .browsing(Box::new(browser.clone()))
    });
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Export…").click();
    step_until(&mut h, "the export dialog", |h| {
        anywhere(h, "Export Untitled")
    });
    browse(&mut h, &browser);
    assert_eq!(
        browser.asked()[0].folder.as_deref(),
        Some(documents.as_path())
    );
    assert_eq!(
        browser.asked()[0].file_name.as_deref(),
        Some("Untitled.3mf")
    );
    browser.answer(None);
    // A path that isn't full: no folder of its own, so the platform's.
    set_field(&mut h, "Path", "out.3mf");
    browse(&mut h, &browser);
    assert_eq!(browser.asked()[1].folder, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_save_as_that_closes_its_part_counts_as_closing_while_it_runs() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    h.get_by_label("Close Untitled").click();
    step_until(&mut h, "the close question", |h| {
        anywhere(h, "Save changes to Untitled before closing?")
    });
    answer_question(&mut h, "Save");
    save_panel_asked(&mut h, &browser, 1);
    let (hold, check) = hold_queue(&f, &h, 0);
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "the Save As sent", |h| {
        h.state().tabs()[0].saving_as()
    });
    assert!(h.state().tabs()[0].closing());
    // Closing: Save As and Export are off, as for a save that closes.
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    for item in ["Save As…", "Export…"] {
        assert!(
            button_off_start(&h, item).accesskit_node().is_disabled(),
            "{item}"
        );
    }
    h.key_press(eframe::egui::Key::Escape);
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "closed", |h| h.state().tabs().is_empty());
    assert_eq!(names_in(&part), ["Box"]);
    assert_eq!(never_waits::waits(), 0);
}

/// `path` typed into the typed Save As's empty Path field.
fn type_path(h: &mut AppHarness, path: &Path) {
    path_field(h).focus();
    step(h);
    path_field(h).type_text(path.to_str().unwrap());
    step(h);
    step(h);
}

// ---- Untitled parts: which part a Save panel, a quit, and Save act on ----
//
// Each of these pins one choice the app makes between two parts, or between a Save panel
// open and one to open: two untitled parts, one with its panel open; a quit with a part's
// panel open, or with a part closed meanwhile; Save while a dialog is open or a Save As
// runs; and an export form open across a part's first save. Expected names, paths, and
// counts of panels asked are written here by hand.

/// The app bar's Save button.
fn bar_save(h: &AppHarness) -> egui_kittest::Node<'_> {
    h.query_all_by_role_and_label(Role::Button, "Save")
        .next()
        .unwrap()
}

#[test]
fn a_second_untitled_parts_save_while_the_firsts_panel_is_open_asks_its_own_typed_save_as() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    new_part(&mut h);
    add_box(&mut h, "Untitled 2");
    // Untitled shown, and Command+S: its panel.
    h.get_by_label("Untitled *").click();
    step(&mut h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    assert_eq!(browser.asked()[0].title, "Save Untitled as");
    // Untitled 2 shown, and Command+S: not the first's panel, which is open (one native
    // dialog at a time), but Untitled 2's own typed Save As.
    h.get_by_label("Untitled 2 *").click();
    step(&mut h);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(
        anywhere(&h, "Save Untitled 2 as"),
        "Untitled 2's typed Save As"
    );
    assert!(!anywhere(&h, "Save Untitled as"));
    assert_eq!(browser.asked().len(), 1);
    // Each saved to its own file: Untitled 2 to the path typed, Untitled to the panel's
    // pick, and neither closed.
    let two = f.dir.path().join("two.silico");
    type_path(&mut h, &two);
    answer_question(&mut h, "Save");
    step_until(&mut h, "Untitled 2 saved", |h| {
        h.state().tabs()[1].path.as_deref() == two.to_str()
    });
    assert_eq!(h.state().tabs()[0].path, None);
    let one = f.dir.path().join("one.silico");
    browser.answer(Some(&one));
    step_until(&mut h, "Untitled saved", |h| {
        h.state().tabs()[0].path.as_deref() == one.to_str()
    });
    let names: Vec<&str> = h.state().tabs().iter().map(|tab| tab.name()).collect();
    assert_eq!(names, ["one.silico", "two.silico"]);
    assert!(h.state().tabs().iter().all(|tab| !tab.unsaved()));
    for path in [&one, &two] {
        assert_eq!(names_in(path), ["Box"], "{}", path.display());
    }
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_quits_save_all_with_a_parts_panel_open_then_cancelled_stops_the_quit_asking_no_second_panel() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        anywhere(h, "Save changes before quitting?")
    });
    answer_question(&mut h, "Save all");
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    // The panel the quit took on, cancelled: the part's close refused, so the quit stops,
    // said, and no second panel is asked for it.
    browser.answer(None);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(
        browser.asked().len(),
        1,
        "a second panel asked for Untitled"
    );
    step_until(&mut h, "the quit stopped", |h| {
        h.state().message() == Some("Quit stopped: Untitled wasn't closed")
    });
    for _ in 0..20 {
        step(&mut h);
        assert!(!told(&h, true), "the window closed with Untitled's edits");
    }
    assert_eq!(browser.asked().len(), 1);
    let tab = &h.state().tabs()[0];
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!((tab.name(), tab.path.as_deref()), ("Untitled", None));
    assert!(tab.unsaved() && !tab.closing());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_quit_passes_over_an_untitled_part_closed_without_saving_while_anothers_panel_was_open() {
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    let before = listing(f.dir.path());
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    new_part(&mut h);
    add_box(&mut h, "Untitled 2");
    close_window(&mut h);
    step_until(&mut h, "the quit question", |h| {
        anywhere(h, "Save changes before quitting?")
    });
    answer_question(&mut h, "Save all");
    save_panel_asked(&mut h, &browser, 1);
    assert_eq!(browser.asked()[0].title, "Save Untitled as");
    // While Untitled's panel is open, Untitled 2 closed, Don't save, its rollback held:
    // it's closing when Untitled's pick comes.
    let (hold, check) = hold_queue(&f, &h, 1);
    h.get_by_label("Close Untitled 2").click();
    step_until(&mut h, "the close question", |h| {
        anywhere(h, "Save changes to Untitled 2 before closing?")
    });
    answer_question(&mut h, "Don't save");
    step_until(&mut h, "Untitled 2 closing", |h| {
        h.state().tabs()[1].closing()
    });
    let one = f.dir.path().join("one.silico");
    browser.answer(Some(&one));
    step_until(&mut h, "Untitled saved", |h| {
        h.state().tabs()[0].path.as_deref() == one.to_str()
    });
    // Untitled 2, its turn come, closing already: passed over, no panel asked for it.
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(h.state().tabs().len(), 1);
    assert_eq!(h.state().tabs()[0].name(), "Untitled 2");
    assert!(h.state().tabs()[0].closing());
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the window closing", |h| told(h, true));
    assert!(h.state().tabs().is_empty());
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(names_in(&one), ["Box"]);
    // No other document written: Untitled 2's edits dropped, as Don't save says.
    let documents = |listed: Vec<String>| -> Vec<String> {
        listed
            .into_iter()
            .filter(|name| name.ends_with(".silico"))
            .collect()
    };
    assert!(documents(before).is_empty());
    assert_eq!(documents(listing(f.dir.path())), ["one.silico"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_untitled_parts_save_is_off_and_command_s_asks_nothing_in_a_dialog_or_while_its_save_as_runs()
{
    let f = fixture();
    let browser = FakeBrowser::default();
    let mut h = f.app_browsing(&browser);
    step(&mut h);
    new_part(&mut h);
    step(&mut h);
    assert!(!bar_save(&h).accesskit_node().is_disabled());
    // A dialog open: Save off, and Command+S asks no panel.
    create(&mut h, "Box");
    step_until(&mut h, "its add answered", |h| {
        tab_named(h, "Untitled")
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    });
    assert!(bar_save(&h).accesskit_node().is_disabled());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(browser.asked().is_empty());
    assert!(dialog_open(&h));
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    step_until(&mut h, "unsaved", |h| {
        tab_named(h, "Untitled").edits.idle() && anywhere(h, "Untitled *")
    });
    assert!(!bar_save(&h).accesskit_node().is_disabled());
    // Its Save As picked and held: neither closing nor a dialog, but saving as. Save off,
    // and Command+S asks no second panel.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    let (hold, check) = hold_queue(&f, &h, 0);
    let part = f.dir.path().join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "the Save As sent", |h| {
        h.state().tabs()[0].saving_as()
    });
    let tab = &h.state().tabs()[0];
    assert!(!tab.closing() && !tab.in_dialog());
    step(&mut h);
    assert!(bar_save(&h).accesskit_node().is_disabled());
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(browser.asked().len(), 1);
    assert!(h.state().tabs()[0].saving_as());
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "saved", |h| {
        h.state().tabs()[0].path.as_deref() == part.to_str() && !h.state().tabs()[0].unsaved()
    });
    assert_eq!(names_in(&part), ["Box"]);
    assert_eq!(browser.asked().len(), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_export_form_opened_while_untitled_takes_a_relative_path_in_the_folder_of_its_first_save() {
    use silico_core::command::export::{Export, ExportFormat};
    let f = fixture();
    let browser = FakeBrowser::default();
    let documents = f.dir.path().join("Documents");
    let mut h = f.app_made(|app| {
        app.documents_folder(Some(documents.clone()))
            .browsing(Box::new(browser.clone()))
    });
    step(&mut h);
    new_part(&mut h);
    add_box(&mut h, "Untitled");
    // Its Save panel open (the portal's leaves the window usable), and Export… opened.
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::S);
    save_panel_asked(&mut h, &browser, 1);
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Export…").click();
    step_until(&mut h, "the export dialog", |h| {
        anywhere(h, "Export Untitled")
    });
    let refusal = "Give a full path: an untitled part has no folder";
    press(&mut h, "STEP");
    set_field(&mut h, "Path", "out.step");
    step(&mut h);
    assert!(anywhere(&h, refusal));
    // The panel's pick: the part's first save, while the form is open. A relative path is
    // in the folder of its file from then on.
    let folder = f.dir.path().join("parts");
    std::fs::create_dir(&folder).unwrap();
    let part = folder.join("part.silico");
    browser.answer(Some(&part));
    step_until(&mut h, "saved", |h| {
        h.state().tabs()[0].path.as_deref() == part.to_str()
    });
    step(&mut h);
    assert!(!anywhere(&h, refusal));
    assert_eq!(path_value(&h), "out.step");
    assert!(anywhere(&h, "Export part.silico"));
    press(&mut h, "Export");
    step_until(&mut h, "the dialog closed", |h| {
        !anywhere(h, "Export part.silico")
    });
    let out = folder.join("out.step");
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
    assert!(!f.dir.path().join("out.step").exists());
    assert_eq!(never_waits::waits(), 0);
}

// ---- Picking and selection (M5.7; docs/design/m5.0-app.md, section 9's *Selection*) ----

/// A box, and a second 80 mm along x: two bodies, each named by the element map as a box
/// names its elements, `<id>.box.Face<n>` and `<id>.box.Edge<n>`, its numbers OCCT's (M2.7).
const TWO_BLOCKS: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "a91cw0de", "type": "box", "v": 1, "name": "Other", "position": [80, 0, 0], "length": 10, "width": 10, "height": 10}
  ]
}
"#;

/// The canvas's pixel under `at`, in points: physical pixels from the viewport's top left,
/// floored, as the selection's picking square centres on it.
fn pixel_at(h: &AppHarness, at: eframe::egui::Pos2) -> [u32; 2] {
    let frame = view(h).frame().unwrap();
    let d = (at - frame.rect.min) * frame.pixels_per_point;
    [d.x.floor() as u32, d.y.floor() as u32]
}

/// A left click on the canvas at `at`, Ctrl (Cmd) held if `ctrl`: its press one frame, its
/// release the next.
fn click_canvas(h: &mut AppHarness, at: eframe::egui::Pos2, ctrl: bool) {
    use eframe::egui::{Event, Modifiers, PointerButton};
    let held = if ctrl {
        Modifiers::COMMAND
    } else {
        Modifiers::NONE
    };
    h.event(Event::ModifiersChanged(held));
    h.hover_at(at);
    button_event(h, at, PointerButton::Primary, true, held);
    step(h);
    button_event(h, at, PointerButton::Primary, false, held);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    step(h);
}

/// What the fake renderer finds at every pick from now on: `hit` (the body, the element,
/// and its pixel), and an edge in the square.
fn answering(
    record: &Record,
    hit: Option<(u32, silico_app::renderer::Element, [u32; 2])>,
    edge: Option<(u32, silico_app::renderer::Element, [u32; 2])>,
) {
    let pick = |(body, hit, pixel)| silico_app::renderer::Pick {
        body,
        hit,
        pixel,
        point: None,
    };
    let mut told = record.lock().unwrap();
    told.pick = hit.map(pick);
    told.edge = edge.map(pick);
}

fn selected_names(h: &AppHarness) -> Vec<String> {
    h.state().tabs()[0]
        .selection()
        .names()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[track_caller]
fn selected_until(h: &mut AppHarness, want: &[&str]) {
    step_until(h, &format!("{want:?} selected"), |h| {
        selected_names(h) == want
    });
}

/// Steps frames until the selection is `want`, each found as a face on the model published
/// now (not waiting for its body's names): bounded as [`step_until`] is.
#[track_caller]
fn found_as_faces_until(h: &mut AppHarness, want: &[&str]) {
    use silico_app::selection::State;
    step_until(h, &format!("{want:?} found as faces"), |h| {
        selected_names(h) == want
            && h.state().tabs()[0].selection().elements().all(|picked| {
                matches!(
                    &picked.state,
                    State::Found(found) if found.kind() == silico_core::kernel::ElementKind::Face
                )
            })
    });
}

/// The body whose feature is `id`, by its number.
fn body_of(h: &AppHarness, id: &str) -> u32 {
    view(h)
        .bodies()
        .iter()
        .find(|(_, body)| body.feature.as_str() == id)
        .map(|(number, _)| *number)
        .unwrap()
}

/// What the last frame given to the renderer highlighted.
fn highlighted(record: &Record) -> Vec<(u32, silico_app::renderer::Element)> {
    record
        .lock()
        .unwrap()
        .frames
        .last()
        .unwrap()
        .highlights
        .clone()
}

#[test]
fn a_click_selects_the_element_under_it_by_name_ctrl_adds_and_removes_and_empty_and_esc_clear() {
    use silico_app::renderer::Element::{Edge, Face};
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    let centre = pixel_at(&h, at);
    // The box's front face under the pointer: selected by its name, highlighted.
    answering(&record, Some((1, Face(3), centre)), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    step(&mut h);
    assert_eq!(highlighted(&record), [(1, Face(3))]);
    // Ctrl, an edge two pixels above in the square: added, in the order picked, the edge
    // taken before the face under the pointer.
    answering(
        &record,
        Some((1, Face(3), centre)),
        Some((1, Edge(5), [centre[0], centre[1] - 2])),
    );
    click_canvas(&mut h, at, true);
    selected_until(&mut h, &["k7f2qx3m.box.Face3", "k7f2qx3m.box.Edge5"]);
    step(&mut h);
    assert_eq!(highlighted(&record), [(1, Face(3)), (1, Edge(5))]);
    // Ctrl on the face again: removed.
    answering(&record, Some((1, Face(3), centre)), None);
    click_canvas(&mut h, at, true);
    selected_until(&mut h, &["k7f2qx3m.box.Edge5"]);
    // Ctrl on nothing: nothing changes. A face found beside the pointer, not under it, is
    // nothing too: only an edge is taken from the square around it.
    answering(
        &record,
        Some((1, Face(3), [centre[0] + 2, centre[1]])),
        None,
    );
    click_canvas(&mut h, at, true);
    for _ in 0..4 {
        step(&mut h);
    }
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Edge5"]);
    // A plain click on it: cleared.
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &[]);
    step(&mut h);
    assert!(highlighted(&record).is_empty());
    // A plain click replaces: the face alone; Esc clears it.
    answering(&record, Some((1, Face(3), centre)), None);
    click_canvas(&mut h, at, true);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    answering(&record, Some((1, Face(1), centre)), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face1"]);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(h.state().tabs()[0].selection().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_clicks_pick_is_asked_again_until_answered_after_passes_that_were_discarded() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    let centre = pixel_at(&h, at);
    answering(&record, Some((1, Face(3), centre)), None);
    // The pointer there first, its hover answered, so the click's pick is the next asked.
    h.hover_at(at);
    for _ in 0..4 {
        step(&mut h);
    }
    // The next three passes that ask are discarded: their requests reach no renderer.
    record.lock().unwrap().discard = 3;
    let frames = record.lock().unwrap().frames.len();
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    // Asked in each frame from the click's until one was answered: the three discarded,
    // then the one answered, each of the same square about the pointer's pixel.
    let told = record.lock().unwrap();
    assert_eq!(told.discard, 0);
    let asked: Vec<_> = told.frames[frames..]
        .iter()
        .filter_map(|frame| frame.picking)
        .collect();
    let square = silico_app::renderer::PixelRect {
        min: [centre[0] as i32 - 3, centre[1] as i32 - 3],
        size: [7, 7],
    };
    assert!(asked.len() >= 4, "{asked:?}");
    assert!(asked.iter().all(|rect| *rect == square), "{asked:?}");
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_hovered_element_is_named_drawn_and_said_and_let_go_off_the_canvas() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    h.hover_at(at);
    step_until(&mut h, "the hover", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
    let selection = h.state().tabs()[0].selection();
    let hovered = selection.hovered().unwrap();
    assert_eq!(hovered.name.to_string(), "k7f2qx3m.box.Face3");
    assert!(hovered.stable);
    assert_eq!(
        hovered.found.as_ref().map(|found| found.name.to_string()),
        Some("k7f2qx3m.box.Face3".to_owned())
    );
    assert_eq!(
        selection.hovered_feature().map(|id| id.as_str()),
        Some("k7f2qx3m")
    );
    // Drawn hovered, not selected; said in the status bar.
    step(&mut h);
    let last = record.lock().unwrap().frames.last().unwrap().clone();
    assert_eq!(last.hovered, [(1, Face(3))]);
    assert!(last.highlights.is_empty());
    assert!(anywhere(&h, "k7f2qx3m.box.Face3"));
    // Off the canvas: let go.
    h.hover_at(eframe::egui::pos2(5.0, 5.0));
    step(&mut h);
    step(&mut h);
    assert!(h.state().tabs()[0].selection().hovered().is_none());
    assert!(
        record
            .lock()
            .unwrap()
            .frames
            .last()
            .unwrap()
            .hovered
            .is_empty()
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_face_picked_marks_and_opens_its_bodys_row_and_a_row_selected_lights_its_bodys_faces() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    step_until(&mut h, "its row", |h| {
        h.get_by_role_and_label(Role::Pane, "Browser")
            .query_by_label("Block")
            .is_some()
    });
    let toggled = |h: &AppHarness, pane: &str| {
        h.get_by_role_and_label(Role::Pane, pane)
            .query_by_label("Block")
            .and_then(|node| node.accesskit_node().toggled())
            .map(|toggled| toggled == eframe::egui::accesskit::Toggled::True)
    };
    // The Bodies group closed: no row.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Collapse Bodies")
        .click();
    step(&mut h);
    step(&mut h);
    assert_eq!(toggled(&h, "Browser"), None);
    // A face picked in the canvas: the group opened, its body's row there and marked;
    // the timeline's item isn't (the selection is a face, not the feature).
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    step_until(&mut h, "its row marked", |h| {
        toggled(h, "Browser") == Some(true)
    });
    assert_eq!(toggled(&h, "Timeline"), Some(false));
    // Its row clicked: the body selected, the face let go, and the canvas lights each of
    // its six faces.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Block")
        .click();
    step_until(&mut h, "the body selected", |h| {
        h.state().tabs()[0]
            .selection()
            .feature()
            .is_some_and(|id| id.as_str() == "k7f2qx3m")
    });
    assert!(selected_names(&h).is_empty());
    step(&mut h);
    step(&mut h);
    let faces: Vec<_> = (1..=6).map(|n| (1, Face(n))).collect();
    assert_eq!(highlighted(&record), faces);
    assert_eq!(toggled(&h, "Timeline"), Some(true));
    assert_eq!(never_waits::waits(), 0);
}

/// Forty boxes, `b0000001` to `b0000040`, named `Body01` to `Body40`: forty rows of 24
/// points, more than the browser shows.
fn forty_blocks() -> String {
    let features: Vec<String> = (1..=40)
        .map(|n| {
            format!(
                r#"{{"id": "b{n:07}", "type": "box", "v": 1, "name": "Body{n:02}", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}}"#
            )
        })
        .collect();
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// Whether the browser's row `name` is drawn within the band the browser shows, top to
/// bottom: the canvas's, which the browser stands beside (its pane's node has no bounds).
fn row_shown(h: &AppHarness, name: &str) -> bool {
    let band = the_viewport(h);
    h.get_by_role_and_label(Role::Pane, "Browser")
        .query_all_by_label(name)
        .filter(|row| row.accesskit_node().bounding_box().is_some())
        .any(|row| {
            let row = row.rect();
            row.top() >= band.top() && row.bottom() <= band.bottom()
        })
}

#[test]
fn the_browser_scrolls_to_a_picked_faces_body_row_and_to_no_other() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("blocks.silico", &forty_blocks());
    meshes_settled(&mut h);
    step_until(&mut h, "its rows", |h| {
        h.get_by_role_and_label(Role::Pane, "Browser")
            .query_by_label("Body40")
            .is_some()
    });
    // Nothing picked: the browser stays at its top, its first row shown, its last not.
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(row_shown(&h, "Body01"));
    assert!(!row_shown(&h, "Body40"));
    // A face of the last body picked: its row scrolled to.
    let at = the_viewport(&h).center();
    let centre = pixel_at(&h, at);
    let last = body_of(&h, "b0000040");
    answering(&record, Some((last, Face(3), centre)), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["b0000040.box.Face3"]);
    step_until(&mut h, "the last row shown", |h| row_shown(h, "Body40"));
    assert!(!row_shown(&h, "Body01"));
    // Then a face of the first: its row, and not another, scrolled to.
    let first = body_of(&h, "b0000001");
    answering(&record, Some((first, Face(3), centre)), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["b0000001.box.Face3"]);
    // Scrolled back as the browser's scroll eases, on state, not a count of frames.
    step_until(&mut h, "the first row shown", |h| row_shown(h, "Body01"));
    assert!(!row_shown(&h, "Body40"));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_picked_faces_row_is_scrolled_to_once_so_its_group_closed_after_stays_closed() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    step(&mut h);
    step(&mut h);
    // Scrolled to, and the scroll's target let go.
    assert!(row_shown(&h, "Block"));
    assert_eq!(h.state().tabs()[0].selection().scroll_target(), None);
    // The Bodies group closed with the face still selected: it stays closed.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Collapse Bodies")
        .click();
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(
        h.get_by_role_and_label(Role::Pane, "Browser")
            .query_by_label("Block")
            .is_none()
    );
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_properties_of_a_selected_face_edge_and_vertex_are_under_its_kind() {
    // The selection view's section names each element's kind (`selectionProps`'s heading):
    // FACE, EDGE, VERTEX, each in capitals as a section is.
    use silico_app::selection::How;
    let f = fixture();
    let (mut h, _) = f.opened("box.silico", A_BOX);
    step_until(&mut h, "the model published", |h| {
        h.state().tabs()[0].published.is_some()
    });
    step(&mut h);
    let section = |h: &AppHarness, kind: &str| {
        h.get_by_role_and_label(Role::Pane, "Properties")
            .query_by_label(kind)
            .is_some()
    };
    for (name, kind) in [
        ("k7f2qx3m.box.Face3", "FACE"),
        ("k7f2qx3m.box.Edge5", "EDGE"),
        ("k7f2qx3m.box.Vertex2", "VERTEX"),
    ] {
        h.state_mut().tabs_mut()[0]
            .selection_mut()
            .pick(name, How::Replace);
        assert_eq!(selected_names(&h), [name]);
        step(&mut h);
        step(&mut h);
        assert!(section(&h, kind), "{name}: no {kind}");
        assert!(!section(&h, "ELEMENT"), "{name}");
    }
    assert_eq!(never_waits::waits(), 0);
}

/// A fuse's history in which its base, a 1 mm cube, is the result, its Face1 and Edge1
/// reported by nothing (Edge1 bounding Face1 alone), so both are named by their numbers,
/// unstable (N3); the rest kept; the tool reported by nothing. By hand, as the core's
/// selection tests make it (`numbering_fuse`).
fn numbering_fuse() -> silico_core::kernel::History {
    use silico_core::kernel::{ElementCounts, ElementFate, History, InputHistory, Topology};
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    let none = |n: usize| vec![ElementFate::default(); n];
    let face1 = |rest: Vec<Vec<u32>>| -> Vec<Vec<u32>> {
        std::iter::once(vec![1, 2]).chain(rest).collect()
    };
    History {
        inputs: vec![
            InputHistory {
                faces: none(1).into_iter().chain((2..=6).map(kept)).collect(),
                edges: none(1).into_iter().chain((2..=12).map(kept)).collect(),
                vertices: (1..=8).map(kept).collect(),
            },
            InputHistory {
                faces: none(6),
                edges: none(12),
                vertices: none(8),
            },
        ],
        result: ElementCounts {
            faces: 6,
            edges: 12,
            vertices: 8,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: face1(vec![vec![]; 5]),
            outer_edges: face1(vec![vec![]; 5]),
            edge_vertices: vec![vec![]; 12],
            solid_faces: vec![(1..=6).collect()],
        },
    }
}

const FUSED: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Base", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1},
    {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Tool", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1},
    {"id": "rrrrrrrr", "type": "fuse", "v": 1, "name": "Fused", "base": "aaaaaaaa", "tool": "bbbbbbbb"}
  ]
}
"#;

#[test]
fn an_unstable_name_is_selectable_shown_with_its_reason_and_refused_by_a_store() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    f.kernel.answer_instead(|op| match op {
        Op::Boolean {
            op: silico_core::kernel::BooleanOp::Fuse,
            base,
            ..
        } => Some(silico_protocol::Reply::Built(
            silico_core::kernel::BuiltData {
                // A box a mm³ larger than the base, so the fuse changes it (S7's rule).
                shape: FakeKernel::resized(base, 1.0)?,
                history: numbering_fuse(),
            },
        )),
        _ => None,
    });
    let (mut h, record) = f.drawn("fused.silico", FUSED);
    meshes_settled(&mut h);
    let body = body_of(&h, "rrrrrrrr");
    let at = the_viewport(&h).center();
    let centre = pixel_at(&h, at);
    // Its Face1, named by its number: selected, as an agent would name it, and drawn.
    answering(&record, Some((body, Face(1), centre)), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["rrrrrrrr.fuse.Face1"]);
    step(&mut h);
    assert_eq!(highlighted(&record), [(body, Face(1))]);
    // Shown with why a store refuses it, in a reference's words (by hand from
    // references.rs's message, N3), at its place in the selection.
    let why = "selection.0, \"rrrrrrrr.fuse.Face1\", names a face, whose name is unstable \
               (it came about as another element did, or is its number): an edit could make \
               it another element";
    step(&mut h);
    assert!(shows(&h, "Properties", "1 element selected"));
    assert!(shows(&h, "Properties", "rrrrrrrr.fuse.Face1"));
    assert!(shows(&h, "Properties", "Unstable"));
    assert!(
        h.get_by_role_and_label(Role::Pane, "Properties")
            .query_all_by_label_contains(why)
            .next()
            .is_some(),
        "{:?}",
        labels(&h, "Properties", Role::Label)
    );
    // Its card a warning's (`.err-card.warn`: `--alert-warn-bg`, rgba(223, 119, 0, .18),
    // bordered #5c3a08), with the refusal's suggestion after its arrow, in the core's words
    // (by hand from selection.rs's refusal of an unstable face).
    let fix = "name the face as its feature now names it: the candidates are the likeliest, \
               none taken for it";
    let fix_at = h
        .get_by_role_and_label(Role::Pane, "Properties")
        .get_by_label(fix)
        .rect();
    let warn = eframe::egui::Color32::from_rgba_unmultiplied(223, 119, 0, 46);
    let warn_border = eframe::egui::Color32::from_rgb(0x5c, 0x3a, 0x08);
    let cards: Vec<_> = painted(&h)
        .into_iter()
        .filter_map(|shape| match shape {
            eframe::egui::Shape::Rect(card) if card.rect.contains_rect(fix_at) => {
                Some((card.fill, card.stroke.color))
            }
            _ => None,
        })
        .collect();
    assert!(cards.contains(&(warn, warn_border)), "{cards:?}");
    let refusal = h.state().tabs()[0].selection().storable().unwrap_err();
    assert_eq!(refusal.code(), ErrorCode::ReferenceBroken);
    assert_eq!(refusal.message(), why);
    assert_eq!(refusal.parameter(), Some("selection.0"));
    // Its Face2, kept from the base and stable, added: the store still refuses, for the
    // first; alone, the store takes it.
    answering(&record, Some((body, Face(2), centre)), None);
    click_canvas(&mut h, at, true);
    selected_until(&mut h, &["rrrrrrrr.fuse.Face1", "aaaaaaaa.box.Face2"]);
    assert_eq!(
        h.state().tabs()[0]
            .selection()
            .storable()
            .unwrap_err()
            .message(),
        why
    );
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["aaaaaaaa.box.Face2"]);
    let stored: Vec<String> = h.state().tabs()[0]
        .selection()
        .storable()
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(stored, ["aaaaaaaa.box.Face2"]);
    step(&mut h);
    assert!(shows(&h, "Properties", "Stable"));
    assert_eq!(never_waits::waits(), 0);
}

/// `id`'s `field` set to `value` through the core, as an agent would, and frames stepped
/// until the tab has read the model it publishes.
fn set_through_the_core(f: &Fixture, h: &mut AppHarness, id: &str, field: &str, value: &str) {
    let session = h.state().tabs()[0].id;
    let revision = f.sessions.status(&session).session.unwrap().revision;
    let models = h.state().tabs()[0].published_models;
    f.sessions
        .set_field(
            &session,
            revision,
            silico_core::document::FeatureId::new(id).unwrap(),
            field.to_owned(),
            json::read(value.as_bytes()).unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(h, "the new model", |h| {
        h.state().tabs()[0].published_models != models
    });
    meshes_settled(h);
}

/// Each element selected, and what the selection says of it: found, or waiting (and why).
fn element_states(h: &AppHarness) -> Vec<(String, Option<String>)> {
    h.state().tabs()[0]
        .selection()
        .elements()
        .map(|picked| {
            let state = match &picked.state {
                silico_app::selection::State::Found(_) => None,
                silico_app::selection::State::Waiting { why, .. } => {
                    Some(why.clone().unwrap_or_else(|| "pending".to_owned()))
                }
            };
            (picked.name.clone(), state)
        })
        .collect()
}

#[test]
fn the_selection_survives_a_rebuild_that_keeps_its_names_waits_on_a_failure_and_says_whats_gone() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    // The block 41 long fails to build.
    f.kernel.refuse(|op| match op {
        Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx == 41.0 => {
            Some(Error::new(ErrorCode::KernelFailed, "the block came apart"))
        }
        _ => None,
    });
    let (mut h, record) = f.drawn("blocks.silico", TWO_BLOCKS);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    let centre = pixel_at(&h, at);
    let other = "a91cw0de.box.Face3";
    let block = "k7f2qx3m.box.Face1";
    answering(
        &record,
        Some((body_of(&h, "a91cw0de"), Face(3), centre)),
        None,
    );
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &[other]);
    answering(
        &record,
        Some((body_of(&h, "k7f2qx3m"), Face(1), centre)),
        None,
    );
    click_canvas(&mut h, at, true);
    selected_until(&mut h, &[other, block]);
    // A rebuild that keeps their names (the other box 12 long): both kept, found, nothing
    // said.
    set_through_the_core(&f, &mut h, "a91cw0de", "length", "12");
    assert_eq!(
        element_states(&h),
        [(other.to_owned(), None), (block.to_owned(), None)]
    );
    assert_eq!(h.state().tabs()[0].selection().said(), None);
    // The block failing: its face waits, saying why (the core's words), kept selected; a
    // store refuses it, at its place.
    set_through_the_core(&f, &mut h, "k7f2qx3m", "length", "41");
    let states = element_states(&h);
    assert_eq!(states[0], (other.to_owned(), None));
    assert_eq!(states[1].0, block);
    let why = states[1].1.clone().unwrap();
    assert!(why.contains("k7f2qx3m"), "{why}");
    let refusal = h.state().tabs()[0].selection().storable().unwrap_err();
    assert_eq!(refusal.parameter(), Some("selection.1"));
    assert_eq!(
        refusal.message(),
        format!("{block} isn't in the model as it's built now: {why}")
    );
    // Built again: found again.
    set_through_the_core(&f, &mut h, "k7f2qx3m", "length", "40");
    assert_eq!(
        element_states(&h),
        [(other.to_owned(), None), (block.to_owned(), None)]
    );
    // The other box removed: its face is gone, dropped, and said, how many of the faces
    // selected went and, for one, why, in the core's words (by hand from its lookup's
    // message for a name of no feature); the block's stays, as SolidWorks' selection box
    // keeps its valid items.
    let session = h.state().tabs()[0].id;
    let revision = f.sessions.status(&session).session.unwrap().revision;
    let models = h.state().tabs()[0].published_models;
    f.sessions
        .remove_feature(
            &session,
            revision,
            silico_core::document::FeatureId::new("a91cw0de").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the new model", |h| {
        h.state().tabs()[0].published_models != models
    });
    selected_until(&mut h, &[block]);
    let said = h.state().tabs()[0].selection().said().unwrap().to_owned();
    let gone = format!(
        "1 of 2 selected faces is gone after the rebuild: selection.0, \"{other}\", isn't a \
         name: no feature a91cw0de in the model, at column 1"
    );
    assert!(said.starts_with(&gone), "{said}");
    step(&mut h);
    assert!(anywhere(&h, &said), "the status bar doesn't say {said}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn on_lavapipe_a_click_on_a_boxs_front_face_selects_it_by_name_and_an_edge_is_taken_within_its_band()
 {
    on_lavapipe(
        "on_lavapipe_a_click_on_a_boxs_front_face_selects_it_by_name_and_an_edge_is_taken_within_its_band",
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
            h.set_render_every_step(true);
            // In the theme, its fonts and icons, as main.rs applies them: for the screenshot.
            silico_app::theme::apply(&h.ctx);
            let path = f.file("box.silico", A_BOX);
            h.state_mut().open(path.to_str().unwrap(), false);
            meshes_settled(&mut h);
            // From the front (−y), onto the box's centre (20, 10, 5), 20 mm high: a point
            // (x, 0, z) of the front face (Face3, y = 0, x 0 to 40, z 0 to 10) is seen
            // (x − 20) mm right of the pane's centre and (z − 5) mm above it, a mm being
            // the pane's height over 20.
            view_mut(&mut h).set_camera(silico_protocol::Camera {
                projection: silico_protocol::Projection::Orthographic { view_height: 20.0 },
                eye: [20.0, -100.0, 5.0],
                target: [20.0, 10.0, 5.0],
                up: [0.0, 0.0, 1.0],
            });
            meshes_settled(&mut h);
            for _ in 0..3 {
                step(&mut h);
            }
            let rect = the_viewport(&h);
            let mm = rect.height() / 20.0;
            let seen = |x: f32, z: f32| {
                rect.center() + eframe::egui::vec2((x - 20.0) * mm, -(z - 5.0) * mm)
            };
            let select_at = |h: &mut AppHarness, at, ctrl, want: &[&str]| {
                click_canvas(h, at, ctrl);
                for _ in 0..12 {
                    if selected_names(h) == want {
                        break;
                    }
                    step(h);
                }
                assert_eq!(selected_names(h), want, "at {at:?}");
            };
            // The face at (30, 0, 3): Face3, by the name the element map gives it.
            select_at(&mut h, seen(30.0, 3.0), false, &["k7f2qx3m.box.Face3"]);
            // Its top edge, z = 10 (drawn 2 points wide about its line), from the face 2
            // points below its line: inside the 3-point band, the edge, not the face. It's
            // Edge3 in the fixture's numbering (`silico_render::bench::box_mesh`: the front
            // face's bottom, right, top, left first), which the fake kernel meshes every box
            // with, so the element map names it so; OCCT's box numbers the top front edge
            // Edge10. The name is the fixture's, not OCCT's.
            let edge = seen(30.0, 10.0);
            let below = |points: f32| edge + eframe::egui::vec2(0.0, points);
            select_at(&mut h, below(2.0), false, &["k7f2qx3m.box.Edge3"]);
            // 6 points below it, past the band: the face under the pointer.
            select_at(&mut h, below(6.0), false, &["k7f2qx3m.box.Face3"]);
            // Above the box (z = 14, clear of the ViewCube at the top right), the
            // background: a click there clears.
            select_at(&mut h, seen(10.0, 14.0), false, &[]);
            // The face and the edge, for the screenshot, with the properties panel's view.
            select_at(&mut h, seen(30.0, 3.0), false, &["k7f2qx3m.box.Face3"]);
            select_at(
                &mut h,
                below(2.0),
                true,
                &["k7f2qx3m.box.Face3", "k7f2qx3m.box.Edge3"],
            );
            // The pointer on the face, hovered: its answer a few frames on, on lavapipe.
            h.hover_at(seen(10.0, 5.0));
            step_until(&mut h, "the face hovered", |h| {
                h.state().tabs()[0]
                    .selection()
                    .hovered()
                    .is_some_and(|hovered| hovered.name.to_string() == "k7f2qx3m.box.Face3")
            });
            step(&mut h);
            assert!(shows(&h, "Properties", "2 elements selected"));
            if let Some(shot) = std::env::var_os("SILICO_APP_SELECTION_SHOT") {
                h.render().unwrap().save(shot).unwrap();
            }
            assert_eq!(never_waits::waits(), 0);
        },
    );
}

/// The fake answers each fuse with `history` of the tool's description (`box(2, …)` for a
/// tool 2 long), giving a box a mm³ larger than the base back, so the fuse changes its base
/// as S7's rule asks.
fn fusing(f: &Fixture, history: fn(&str) -> silico_core::kernel::History) {
    f.kernel.answer_instead(move |op| match op {
        Op::Boolean {
            op: silico_core::kernel::BooleanOp::Fuse,
            base,
            tool,
        } => Some(silico_protocol::Reply::Built(
            silico_core::kernel::BuiltData {
                shape: FakeKernel::resized(base, 1.0)?,
                history: history(&FakeKernel::describe(tool)?),
            },
        )),
        _ => None,
    });
}

/// The numbering fuse after its tool is made 2 long: the base's Face2 kept as the result's
/// Face3, and its Face3 as the result's Face2, so each of their stable names finds another
/// number than before.
fn swapping_fuse() -> silico_core::kernel::History {
    use silico_core::kernel::ElementFate;
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    let mut history = numbering_fuse();
    history.inputs[0].faces = vec![
        ElementFate::default(),
        kept(3),
        kept(2),
        kept(4),
        kept(5),
        kept(6),
    ];
    history
}

/// The tab's published model's count, and the session's revision.
fn models_and_revision(f: &Fixture, h: &AppHarness) -> (u64, silico_core::command::Revision) {
    let session = h.state().tabs()[0].id;
    (
        h.state().tabs()[0].published_models,
        f.sessions.status(&session).session.unwrap().revision,
    )
}

#[test]
fn a_model_published_before_its_meshes_are_read_names_no_pick_and_draws_no_element_till_they_are() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    fusing(&f, |tool| {
        if tool.contains("box(2,") {
            swapping_fuse()
        } else {
            numbering_fuse()
        }
    });
    // The viewport's clock stopped: the click's wait never reaches its backstop
    // (CLICK_WAIT), however slow the machine.
    let (_time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("fused.silico", FUSED, clock);
    meshes_settled(&mut h);
    let body = body_of(&h, "rrrrrrrr");
    let at = the_viewport(&h).center();
    // The face numbered 2 under the pointer, in these meshes and in the next model's: the
    // base's Face2 now, its Face3 once the tool is 2 long.
    answering(&record, Some((body, Face(2), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["aaaaaaaa.box.Face2"]);
    step(&mut h);
    assert_eq!(highlighted(&record), [(body, Face(2))]);
    // The tool made 2 long, the new model's meshes held: it's published, and the view
    // still holds the old model's meshes.
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let (models, revision) = models_and_revision(&f, &h);
    let session = h.state().tabs()[0].id;
    f.sessions
        .set_field(
            &session,
            revision,
            silico_core::document::FeatureId::new("bbbbbbbb").unwrap(),
            "length".to_owned(),
            json::read(b"2").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the new model", |h| {
        h.state().tabs()[0].published_models != models
    });
    // Carried to it, the face selected is the new model's Face3.
    let number = |h: &AppHarness| {
        h.state().tabs()[0]
            .selection()
            .elements()
            .next()
            .and_then(|picked| picked.found())
            .map(|found| found.element.index)
    };
    step_until(&mut h, "the face carried", |h| number(h) == Some(3));
    for _ in 0..4 {
        step(&mut h);
    }
    assert!(view(&h).reading(), "the new model's meshes weren't held");
    // Its number in the new model isn't drawn on the old model's meshes, whose Face3 is
    // another face.
    assert_eq!(highlighted(&record), []);
    // Let go, and a click on the face numbered 2 of the old meshes: named from neither
    // model, it waits. The edit above is another client's, announced in the purple banner,
    // so the first Esc dismisses it, the face staying (the prototype's `escChain`), and the
    // second lets the face go.
    assert!(h.state().tabs()[0].reloaded.is_some());
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(h.state().tabs()[0].reloaded.is_none());
    assert_eq!(selected_names(&h), ["aaaaaaaa.box.Face2"]);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    assert!(selected_names(&h).is_empty());
    click_canvas(&mut h, at, false);
    for _ in 0..10 {
        step(&mut h);
        assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    }
    // The meshes read: the click picks again, of them, and the face numbered 2 is the new
    // model's, the base's Face3, drawn where it is.
    hold.release();
    step_until(&mut h, "the new model's meshes", meshes_current);
    selected_until(&mut h, &["aaaaaaaa.box.Face3"]);
    step(&mut h);
    step(&mut h);
    assert_eq!(highlighted(&record), [(body_of(&h, "rrrrrrrr"), Face(2))]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_unstable_name_picked_is_dropped_by_a_rebuild_saying_why_in_a_references_words() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    fusing(&f, |_| numbering_fuse());
    let (mut h, record) = f.drawn("fused.silico", FUSED);
    meshes_settled(&mut h);
    let body = body_of(&h, "rrrrrrrr");
    let at = the_viewport(&h).center();
    answering(&record, Some((body, Face(1), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["rrrrrrrr.fuse.Face1"]);
    // A rebuild through the core, the fuse's history the same: the text may name another
    // face now, so carried, it's gone, said in a reference's words (references.rs's, N3,
    // by hand), never kept.
    set_through_the_core(&f, &mut h, "bbbbbbbb", "length", "2");
    selected_until(&mut h, &[]);
    let said = h.state().tabs()[0].selection().said().unwrap().to_owned();
    let gone = "The selected face is gone after the rebuild: selection.0, \
                \"rrrrrrrr.fuse.Face1\", names a face, whose name is unstable (it came about as \
                another element did, or is its number): an edit could make it another element";
    assert!(said.starts_with(gone), "{said}");
    assert_eq!(never_waits::waits(), 0);
}

/// A picking square's centre pixel (its sides are odd).
fn square_centre(rect: silico_app::renderer::PixelRect) -> [i32; 2] {
    [0, 1].map(|k| rect.min[k] + i32::try_from(rect.size[k] / 2).unwrap())
}

/// The serial of the last frame the renderer was given, and whether it asked a pick.
fn last_frame(record: &Record) -> (u64, bool) {
    let told = record.lock().unwrap();
    let frame = told.frames.last().unwrap();
    (frame.serial, frame.picking.is_some())
}

#[test]
fn a_hover_answer_to_a_frame_before_a_click_is_not_the_clicks() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let click = the_viewport(&h).center();
    let hover = click - eframe::egui::vec2(40.0, 0.0);
    let (click_px, hover_px) = (pixel_at(&h, click), pixel_at(&h, hover));
    // A renderer that finds Face1 under the hover's pixel and Face3 under the click's,
    // each at its square's centre.
    record.lock().unwrap().by_rect = Some(Box::new(move |rect| {
        let centre = square_centre(rect).map(|c| u32::try_from(c).unwrap());
        let hit = if centre == hover_px {
            Face(1)
        } else if centre == click_px {
            Face(3)
        } else {
            return None;
        };
        Some(silico_app::renderer::Pick {
            body: 1,
            hit,
            pixel: centre,
            point: None,
        })
    }));
    h.hover_at(click + eframe::egui::vec2(0.0, 60.0));
    for _ in 0..4 {
        step(&mut h);
    }
    // Answers held back: the pointer onto the hover's pixel, its pick asked and held; then a
    // click at another pixel, asking its own pick each frame (dropped while one is held).
    record.lock().unwrap().hold = true;
    let before = record.lock().unwrap().frames.len();
    h.hover_at(hover);
    step(&mut h);
    click_canvas(&mut h, click, false);
    step(&mut h);
    let centres = |record: &Record| -> Vec<[u32; 2]> {
        record.lock().unwrap().frames[before..]
            .iter()
            .filter_map(|frame| frame.picking)
            .map(|rect| square_centre(rect).map(|c| u32::try_from(c).unwrap()))
            .collect()
    };
    let asked = centres(&record);
    assert_eq!(asked.first(), Some(&hover_px), "{asked:?}");
    assert!(
        asked[1..].iter().filter(|c| **c == click_px).count() >= 2,
        "the click isn't asking: {asked:?}"
    );
    assert!(selected_names(&h).is_empty());
    // Let go: the hover's answer comes first, to a frame before the click's. Not the
    // click's, which takes the answer to its own pick, Face3, never the hover's Face1.
    record.lock().unwrap().hold = false;
    for _ in 0..8 {
        step(&mut h);
        assert_ne!(selected_names(&h), ["k7f2qx3m.box.Face1"]);
    }
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_hover_answer_to_a_frame_before_the_camera_moved_is_passed_over() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    h.hover_at(eframe::egui::pos2(5.0, 5.0));
    for _ in 0..3 {
        step(&mut h);
    }
    let hovered = |h: &AppHarness| h.state().tabs()[0].selection().hovered().is_some();
    assert!(!hovered(&h));
    // The pointer onto the face: its hover's pick asked, carried by the next frame, in
    // which the view is panned a millimetre (its meshes serve it as they are: nothing is
    // read again).
    h.hover_at(at);
    step(&mut h);
    let mut camera = view(&h).camera().clone();
    camera.eye[0] += 1.0;
    camera.target[0] += 1.0;
    let reads = view(&h).reads();
    view_mut(&mut h).set_camera(camera);
    step(&mut h);
    assert!(last_frame(&record).1, "the hover's pick");
    // Its answer, of the view before the pan: what's under the pointer now may be another
    // face, so it isn't taken.
    for _ in 0..6 {
        step(&mut h);
        assert!(!hovered(&h));
    }
    assert_eq!(view(&h).reads(), reads, "the pan read the meshes again");
    // The pointer moved: asked again, and taken.
    let moved = at + eframe::egui::vec2(1.0, 0.0);
    answering(&record, Some((1, Face(3), pixel_at(&h, moved))), None);
    h.hover_at(moved);
    step_until(&mut h, "the hover", hovered);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn no_hover_is_asked_while_a_drag_navigates_and_it_is_after() {
    use eframe::egui::{Modifiers, PointerButton};
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    h.hover_at(at);
    step_until(&mut h, "the hover", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
    // The middle button dragged 60 points, 10 a frame: the view pans, and no hover is asked
    // while it does, though the pointer moves to another pixel each frame, and none is
    // held.
    button_event(&h, at, PointerButton::Middle, true, Modifiers::NONE);
    step(&mut h);
    let mut asked = Vec::new();
    for i in 1..=6u8 {
        h.hover_at(at + eframe::egui::vec2(10.0 * f32::from(i), 0.0));
        step(&mut h);
        asked.push(last_frame(&record).1);
        if i > 1 {
            assert!(view(&h).navigating());
            assert!(h.state().tabs()[0].selection().hovered().is_none());
        }
    }
    // The press's pixel was asked before it, and the drag is under way from the first
    // move's frame: no frame of the drag carries a pick.
    assert_eq!(asked, [false; 6]);
    let end = at + eframe::egui::vec2(60.0, 0.0);
    button_event(&h, end, PointerButton::Middle, false, Modifiers::NONE);
    step(&mut h);
    // Let go: asked again where the pointer is, and taken.
    answering(&record, Some((1, Face(3), pixel_at(&h, end))), None);
    step_until(&mut h, "the hover again", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_a_renderer_never_answers_is_let_go_after_its_frames() {
    use silico_app::renderer::Element::Face;
    use silico_app::selection::CLICK_FRAMES;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    h.hover_at(at);
    for _ in 0..4 {
        step(&mut h);
    }
    record.lock().unwrap().silent = true;
    let pressed = record.lock().unwrap().frames.len();
    click_canvas(&mut h, at, false);
    for _ in 0..CLICK_FRAMES + 20 {
        step(&mut h);
    }
    // Asked in each of CLICK_FRAMES frames from the click's, then let go, the hover asked
    // once more where the pointer is (the click's asks took its place); then nothing. (The
    // pointer's pixel was asked before: nothing else asks from the press on.)
    let asks: Vec<bool> = record.lock().unwrap().frames[pressed..]
        .iter()
        .map(|frame| frame.picking.is_some())
        .collect();
    let first = asks.iter().position(|asked| *asked).unwrap();
    let n = usize::try_from(CLICK_FRAMES).unwrap() + 1;
    assert!(
        asks[first..first + n].iter().all(|asked| *asked),
        "{asks:?}"
    );
    assert!(!asks[first + n..].iter().any(|asked| *asked), "{asks:?}");
    // The renderer answering now: the click's gone, so nothing is selected.
    {
        let mut told = record.lock().unwrap();
        told.silent = false;
    }
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    h.hover_at(at + eframe::egui::vec2(1.0, 0.0));
    for _ in 0..6 {
        step(&mut h);
    }
    assert!(selected_names(&h).is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn esc_before_a_clicks_answer_lets_the_click_go_with_the_selection() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    let centre = pixel_at(&h, at);
    answering(&record, Some((1, Face(1), centre)), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face1"]);
    // Another click, its pick asked as it's let go; Esc the next frame, before the answer
    // comes: the selection cleared, and the answer selects nothing after it.
    answering(&record, Some((1, Face(3), centre)), None);
    click_canvas(&mut h, at, false);
    h.key_press(eframe::egui::Key::Escape);
    step(&mut h);
    for _ in 0..6 {
        step(&mut h);
        assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    }
    assert_eq!(never_waits::waits(), 0);
}

/// The box's `field` set to `value` through the core, its meshes held by `hold`'s caller
/// ([`published_of_with_meshes_held`]).
fn published_with_meshes_held(f: &Fixture, h: &mut AppHarness, field: &str, value: &str) {
    published_of_with_meshes_held(f, h, "k7f2qx3m", field, value);
}

/// Feature `id`'s `field` set to `value` through the core, its meshes held by `hold`'s
/// caller: the model it publishes waited for in the core, no frame stepped meanwhile, then
/// one frame, in which the tab takes it and starts its read, held. The view still holds the
/// model before's meshes. One frame, so a pick asked before it is still answered after
/// (the viewport waits eight frames for a pick's answer).
fn published_of_with_meshes_held(
    f: &Fixture,
    h: &mut AppHarness,
    id: &str,
    field: &str,
    value: &str,
) {
    published_of_with_meshes_held_once(f, h, id, field, value, |_| true);
}

/// [`published_of_with_meshes_held`], its frame taken only once the session's published
/// model is `ready` (its names settled, say): so the app reads the model in that frame as
/// it is then.
fn published_of_with_meshes_held_once(
    f: &Fixture,
    h: &mut AppHarness,
    id: &str,
    field: &str,
    value: &str,
    ready: impl Fn(&silico_core::recompute::Model) -> bool,
) {
    let (models, revision) = models_and_revision(f, h);
    let session = h.state().tabs()[0].id;
    let edited = f
        .sessions
        .set_field(
            &session,
            revision,
            silico_core::document::FeatureId::new(id).unwrap(),
            field.to_owned(),
            json::read(value.as_bytes()).unwrap(),
        )
        .wait();
    edited.result.unwrap();
    let after = edited.session.unwrap().revision;
    let deadline = Instant::now() + WAIT;
    while f
        .sessions
        .published(&session)
        .result
        .ok()
        .is_none_or(|published| published.revision != after || !ready(&published.model))
    {
        assert!(
            Instant::now() < deadline,
            "the edit's model was never published, or never ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    step(h);
    assert_ne!(h.state().tabs()[0].published_models, models);
    assert!(view(h).reading(), "the new model's read wasn't started");
    assert_ne!(
        view(h).bodies_of().map(|(models, _)| models),
        Some(h.state().tabs()[0].published_models),
        "the new model's meshes weren't held"
    );
}

/// A clock that says what the test sets, from now: frozen until [`advance`]d.
fn hand_clock() -> (Arc<Mutex<Instant>>, Clock) {
    let time = Arc::new(Mutex::new(Instant::now()));
    let read = Arc::clone(&time);
    (time, Arc::new(move || *read.lock().unwrap()))
}

fn advance(time: &Mutex<Instant>, by: Duration) {
    *time.lock().unwrap() += by;
}

/// The delay the last frame asked for the next one at.
fn repaint_delay(h: &AppHarness) -> Option<Duration> {
    h.output()
        .viewport_output
        .get(&eframe::egui::ViewportId::ROOT)
        .map(|viewport| viewport.repaint_delay)
}

/// Whether the view's meshes are of the model the tab has published (the selection's).
fn meshes_current(h: &AppHarness) -> bool {
    let tab = &h.state().tabs()[0];
    tab.published.is_some()
        && view(h).bodies_of()
            == tab
                .published
                .map(|revision| (tab.published_models, revision))
}

/// What the selection says in the status bar.
fn selection_said(h: &AppHarness) -> Option<String> {
    h.state().tabs()[0].selection().said().map(str::to_owned)
}

/// Frames stepped until the view's meshes are the published model's, then until the
/// hover's pick is answered on them: a click waiting would have asked first, and been
/// answered by then.
fn hovered_on_current_meshes(h: &mut AppHarness) {
    step_until(h, "the new model's meshes", meshes_current);
    step_until(h, "the hover on them", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
}

/// The camera panned a millimetre along x, as a control of the person's pans it.
fn pan(h: &mut AppHarness) {
    let mut camera = view(h).camera().clone();
    camera.eye[0] += 1.0;
    camera.target[0] += 1.0;
    view_mut(h).set_camera(camera);
}

#[test]
fn a_click_waiting_for_a_new_models_meshes_is_let_go_by_a_pan_and_the_selection_kept() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    // The viewport's clock stopped: the click's wait never reaches its backstop
    // (CLICK_WAIT), however slow the machine.
    let (_time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("box.silico", A_BOX, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    // The box made 50 long, its meshes held: published, the view still holds the 40 mm
    // box's. A click on what the renderer now calls Face1 waits for the new meshes.
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    published_with_meshes_held(&f, &mut h, "length", "50");
    answering(&record, Some((1, Face(1), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    for _ in 0..4 {
        step(&mut h);
        assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    }
    // Panned while it waits: its pixel was of the view before, so it's let go. The meshes
    // read: nothing is picked, and the selection is as it was.
    pan(&mut h);
    step(&mut h);
    hold.release();
    meshes_settled(&mut h);
    for _ in 0..8 {
        step(&mut h);
        assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    }
    assert_eq!(h.state().tabs()[0].selection().said(), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_waiting_for_a_slow_read_of_a_new_models_meshes_is_picked_on_them_however_many_frames_go_by()
 {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    // The viewport's clock stopped: the click's wait never reaches its backstop
    // (CLICK_WAIT), however slow the machine.
    let (_time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("box.silico", A_BOX, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(1), pixel_at(&h, at))), None);
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    published_with_meshes_held(&f, &mut h, "length", "50");
    click_canvas(&mut h, at, false);
    // A slow read on a busy machine: 200 frames, its answer still held. Nothing the click
    // was aimed at changed, so it waits, asking for no frame of its own (the read's answer
    // wakes the app).
    for _ in 0..200 {
        step(&mut h);
    }
    assert!(
        view(&h).reading(),
        "the meshes came before the frames went by"
    );
    assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    assert_eq!(selection_said(&h), None);
    for _ in 0..10 {
        let woken = h.state().wakes();
        step(&mut h);
        assert!(!asked_at_once(&h) || h.state().wakes() != woken);
    }
    // The meshes read: the click is picked on them.
    hold.release();
    selected_until(&mut h, &["k7f2qx3m.box.Face1"]);
    assert_eq!(selection_said(&h), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_waiting_for_a_models_meshes_is_let_go_and_said_once_another_model_is_published() {
    use silico_app::renderer::Element::Face;
    use silico_app::selection::CLICK_REPUBLISHED;
    let f = fixture();
    meshes_a_real_box(&f);
    // The viewport's clock stopped: the click's wait never reaches its backstop
    // (CLICK_WAIT), however slow the machine.
    let (_time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("box.silico", A_BOX, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    // The box made 50 long, its meshes held; a click waits for them.
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    published_with_meshes_held(&f, &mut h, "length", "50");
    answering(&record, Some((1, Face(1), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    assert_eq!(selection_said(&h), None);
    // Made 60 long before they come: what it would be picked on isn't what it waited for.
    published_with_meshes_held(&f, &mut h, "length", "60");
    assert_eq!(selection_said(&h).as_deref(), Some(CLICK_REPUBLISHED));
    // The 60 mm box's meshes read and picked on: the click is gone, the selection kept.
    hold.release();
    hovered_on_current_meshes(&mut h);
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    assert_eq!(selection_said(&h).as_deref(), Some(CLICK_REPUBLISHED));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn what_a_rebuild_left_gone_is_said_over_a_click_let_go_by_the_same_model() {
    use silico_app::selection::How;
    let f = fixture();
    fusing(&f, |_| numbering_fuse());
    // The viewport's clock stopped: the click's wait never reaches its backstop
    // (CLICK_WAIT), however slow the machine.
    let (_time, clock) = hand_clock();
    let (mut h, _record) = f.drawn_clocked("fused.silico", FUSED, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    // The tool 2 long, its meshes held; the fuse's Face1 (unstable: its number) selected
    // in that model, and a click waiting for its meshes.
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    published_of_with_meshes_held(&f, &mut h, "bbbbbbbb", "length", "2");
    h.state_mut().tabs_mut()[0]
        .selection_mut()
        .pick("rrrrrrrr.fuse.Face1", How::Replace);
    assert_eq!(selected_names(&h), ["rrrrrrrr.fuse.Face1"]);
    // Found as a face on the tool 2 long, once the background has named it: so it's a face
    // that's gone, not an element the selection never had an answer for.
    found_as_faces_until(&mut h, &["rrrrrrrr.fuse.Face1"]);
    click_canvas(&mut h, at, true);
    // The tool 3 long, its frame taken once the background has named the fuse in it: in
    // that one frame, the click is let go and the face is gone; what's said is that, the
    // carry's word said over the click's.
    let fuse = silico_core::document::FeatureId::new("rrrrrrrr").unwrap();
    published_of_with_meshes_held_once(&f, &mut h, "bbbbbbbb", "length", "3", |model| {
        model.named_element_map(&fuse).is_some()
    });
    assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    let said = selection_said(&h).unwrap();
    assert!(
        said.starts_with("The selected face is gone after the rebuild: selection.0, "),
        "{said}"
    );
    hold.release();
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_waiting_for_a_new_models_meshes_is_let_go_by_a_filter_change() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    // The viewport's clock stopped: the click's wait never reaches its backstop
    // (CLICK_WAIT), however slow the machine.
    let (_time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("box.silico", A_BOX, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    published_with_meshes_held(&f, &mut h, "length", "50");
    answering(&record, Some((1, Face(1), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    // Edges no longer picked while it waits (it would pick Face1 still, but by another
    // filter than the one it was made under): let go, the person's own change, unsaid.
    h.state_mut().tabs_mut()[0].selection_mut().filter.edges = false;
    step(&mut h);
    hold.release();
    hovered_on_current_meshes(&mut h);
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    assert_eq!(selection_said(&h), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_on_meshes_whose_read_was_refused_is_let_go_at_once_and_said() {
    use silico_app::renderer::Element::Face;
    use silico_app::selection::CLICK_UNREAD;
    use silico_app::viewport::RETRY_MOST;
    let f = fixture();
    meshes_a_real_box(&f);
    let failing = Arc::new(AtomicBool::new(false));
    let refusing = Arc::clone(&failing);
    f.kernel.refuse(move |op| {
        (matches!(op, Op::Tessellate { .. }) && refusing.load(Ordering::SeqCst))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the worker crashed meshing"))
    });
    // The viewport's clock stopped: the retry its refusal sets can't come due between
    // the click's frames, however slow the machine.
    let (time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("box.silico", A_BOX, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    // The box made 50 long, its meshes refused: none are coming (till a retry).
    failing.store(true, Ordering::SeqCst);
    let (_, revision) = models_and_revision(&f, &h);
    let session = h.state().tabs()[0].id;
    f.sessions
        .set_field(
            &session,
            revision,
            silico_core::document::FeatureId::new("k7f2qx3m").unwrap(),
            "length".to_owned(),
            json::read(b"50").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the new model's read refused", |h| {
        h.state().tabs()[0]
            .published
            .is_some_and(|published| published != revision && view(h).refused_of(published))
    });
    assert!(!meshes_current(&h));
    // A click on the old meshes: let go at once, and said.
    answering(&record, Some((1, Face(1), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    assert_eq!(selection_said(&h).as_deref(), Some(CLICK_UNREAD));
    // Read once the kernel meshes again and the retry is due: the click is gone, the
    // selection kept.
    failing.store(false, Ordering::SeqCst);
    advance(&time, RETRY_MOST);
    hovered_on_current_meshes(&mut h);
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_waiting_on_a_read_that_never_answers_is_let_go_at_its_backstop_and_said() {
    use silico_app::renderer::Element::Face;
    use silico_app::selection::{CLICK_STALE, CLICK_WAIT};
    let f = fixture();
    meshes_a_real_box(&f);
    let (time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("box.silico", A_BOX, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(1), pixel_at(&h, at))), None);
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    published_with_meshes_held(&f, &mut h, "length", "50");
    click_canvas(&mut h, at, false);
    // Its clock frozen, nothing's let go however many frames go by; a frame is asked for
    // at the backstop, not before.
    for _ in 0..10 {
        step(&mut h);
    }
    let short = Duration::from_millis(300);
    advance(&time, CLICK_WAIT - short);
    step(&mut h);
    assert_eq!(selection_said(&h), None);
    // About 300 ms: egui takes a frame off what's asked.
    let delay = repaint_delay(&h).unwrap();
    assert!(
        delay <= short && delay > short - Duration::from_millis(20),
        "{delay:?}"
    );
    // At it: let go, and said, and no frame asked for it any more.
    advance(&time, short);
    step(&mut h);
    assert_eq!(selection_said(&h).as_deref(), Some(CLICK_STALE));
    assert!(view(&h).reading(), "the meshes came before the backstop");
    step(&mut h);
    let delay = repaint_delay(&h).unwrap();
    assert!(delay > Duration::from_secs(1), "{delay:?}");
    // The meshes read and picked on: nothing is selected.
    hold.release();
    hovered_on_current_meshes(&mut h);
    assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_clicks_answer_that_comes_once_a_new_model_is_published_names_nothing_till_its_meshes_come() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    // The viewport's clock stopped: the click's wait never reaches its backstop
    // (CLICK_WAIT), however slow the machine.
    let (_time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("box.silico", A_BOX, clock);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    // The pointer's pixel asked by the hover and answered first, so the next pick asked
    // there is the click's. Then answers held back: a click on the meshes of the model the
    // selection is read against, its pick asked and held, the renderer finding Face3 under
    // it.
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    h.hover_at(at);
    step_until(&mut h, "the hover", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
    record.lock().unwrap().hold = true;
    let before = record.lock().unwrap().frames.len();
    click_canvas(&mut h, at, false);
    step(&mut h);
    let held = record.lock().unwrap().picking.map(|(serial, _)| serial);
    let asked: Vec<u64> = record.lock().unwrap().frames[before..]
        .iter()
        .filter(|frame| frame.picking.is_some())
        .map(|frame| frame.serial)
        .collect();
    // The held pick is the click's: the first asked since the press, by the release's frame.
    assert_eq!(held, asked.first().copied(), "{asked:?}");
    assert!(!asked.is_empty());
    // A new model published, its meshes held; then the click's answer comes, of the old
    // meshes: named from neither model, it isn't taken.
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    published_with_meshes_held(&f, &mut h, "length", "50");
    record.lock().unwrap().hold = false;
    for _ in 0..6 {
        step(&mut h);
        assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    }
    assert!(
        record.lock().unwrap().picking.is_none(),
        "the held answer wasn't given"
    );
    // Taken by the view (within its frames for a pick's answer), not dropped unseen.
    assert_eq!(view(&h).picked().map(|picking| picking.frame), held);
    // The new meshes read: the click is asked again, of them (the camera unmoved), and the
    // renderer's answer now, Face1, selected.
    answering(&record, Some((1, Face(1), pixel_at(&h, at))), None);
    hold.release();
    step_until(&mut h, "the new model's meshes", meshes_current);
    selected_until(&mut h, &["k7f2qx3m.box.Face1"]);
    assert_eq!(never_waits::waits(), 0);
}

// ---- Box and crossing selection (M5.7; docs/design/m5.0-app.md, section 9's *Box
// selection*) ----

/// The real box, 40 by 20 by 10 mm from the origin (`meshes_a_real_box`), from the front
/// (−y), orthographic, 40 mm high about (20, 10, 5), its meshes read for that view.
fn box_from_the_front(f: &Fixture) -> (AppHarness, Record) {
    meshes_a_real_box(f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(silico_protocol::Camera {
        projection: silico_protocol::Projection::Orthographic { view_height: 40.0 },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    });
    for _ in 0..3 {
        step(&mut h);
    }
    step_until(&mut h, "the meshes read", |h| {
        !view(h).reading() && meshes_current(h)
    });
    (h, record)
}

/// Where the box's point `(x, z)` (any y) lands on the canvas, in points: by hand, the
/// canvas's middle is (20, 5), and its height in pixels is 40 mm, so a millimetre is the
/// height over 40, x right and z up.
fn on_canvas(h: &AppHarness, x: f64, z: f64) -> eframe::egui::Pos2 {
    let frame = view(h).frame().unwrap();
    let [w, height] = frame.size_px.map(f64::from);
    let mm = height / 40.0;
    let px = w / 2.0 + (x - 20.0) * mm;
    let py = height / 2.0 - (z - 5.0) * mm;
    let ppp = f64::from(frame.pixels_per_point);
    frame.rect.min + eframe::egui::vec2((px / ppp) as f32, (py / ppp) as f32)
}

/// A left drag on the canvas from `from` to `to` (points), `held` throughout: the pointer
/// there first, its hover answered, then the press, four moves a frame apart, and the
/// release.
fn box_drag(
    h: &mut AppHarness,
    from: eframe::egui::Pos2,
    to: eframe::egui::Pos2,
    held: eframe::egui::Modifiers,
) {
    box_press(h, from, to, held);
    box_release(h, to, held);
}

/// [`box_drag`]'s press and moves, still held.
fn box_press(
    h: &mut AppHarness,
    from: eframe::egui::Pos2,
    to: eframe::egui::Pos2,
    held: eframe::egui::Modifiers,
) {
    use eframe::egui::{Event, PointerButton};
    h.event(Event::ModifiersChanged(held));
    h.hover_at(from);
    for _ in 0..3 {
        step(h);
    }
    button_event(h, from, PointerButton::Primary, true, held);
    step(h);
    for i in 1..=4u8 {
        h.hover_at(from + (to - from) * f32::from(i) / 4.0);
        step(h);
    }
}

/// [`box_drag`]'s release.
fn box_release(h: &mut AppHarness, to: eframe::egui::Pos2, held: eframe::egui::Modifiers) {
    use eframe::egui::{Event, Modifiers, PointerButton};
    button_event(h, to, PointerButton::Primary, false, held);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    step(h);
    step(h);
}

fn box_edges(numbers: &[u32]) -> Vec<String> {
    numbers
        .iter()
        .map(|n| format!("k7f2qx3m.box.Edge{n}"))
        .collect()
}

#[test]
fn a_box_from_empty_space_takes_the_edges_wholly_inside_and_dragged_back_those_it_crosses() {
    use eframe::egui::Modifiers;
    let f = fixture();
    let (mut h, _record) = box_from_the_front(&f);
    // Nothing under the pointer (the fake renderer finds nothing): a box. From (−2, −2) to
    // (42, 2) mm round the box's foot, left to right: a window, so the edges wholly inside,
    // by hand from `box_mesh`'s order: the front and back feet (1, 5), and the two along y
    // at the foot's ends (9, 10), each a point here. Not the uprights (2, 4, 6, 8), which
    // leave it.
    let low = on_canvas(&h, -2.0, -2.0);
    let high = on_canvas(&h, 42.0, 2.0);
    box_drag(&mut h, low, high, Modifiers::NONE);
    assert_eq!(selected_names(&h), box_edges(&[1, 5, 9, 10]));
    // A toast counts them, as the prototype's `onBox` words it.
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some("Window selection: 4 edges")
    );
    // The same box, right to left: a crossing, so the uprights it crosses too.
    let (right_low, left_high) = (on_canvas(&h, 42.0, -2.0), on_canvas(&h, -2.0, 2.0));
    box_drag(&mut h, right_low, left_high, Modifiers::NONE);
    assert_eq!(selected_names(&h), box_edges(&[1, 2, 4, 5, 6, 8, 9, 10]));
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some("Crossing selection: 8 edges")
    );
    // Shift adds the window round the top (8 to 12 mm up): the top edges (3, 7) and those
    // along y at its ends (11, 12), after what was selected.
    let (top_low, top_high) = (on_canvas(&h, -2.0, 8.0), on_canvas(&h, 42.0, 12.0));
    box_drag(&mut h, top_low, top_high, Modifiers::SHIFT);
    assert_eq!(
        selected_names(&h),
        box_edges(&[1, 2, 4, 5, 6, 8, 9, 10, 3, 7, 11, 12])
    );
    // Ctrl inverts inside a window round the right side (38 to 42 mm): its uprights (2, 6)
    // and the ends along y (10, 11), all selected, so removed; the rest kept.
    let (side_low, side_high) = (on_canvas(&h, 38.0, -2.0), on_canvas(&h, 42.0, 12.0));
    box_drag(&mut h, side_low, side_high, Modifiers::COMMAND);
    assert_eq!(selected_names(&h), box_edges(&[1, 4, 5, 8, 9, 3, 7, 12]));
    // And again: added back, at the end.
    box_drag(&mut h, side_low, side_high, Modifiers::COMMAND);
    assert_eq!(
        selected_names(&h),
        box_edges(&[1, 4, 5, 8, 9, 3, 7, 12, 2, 6, 10, 11])
    );
    // A plain box round nothing, over the box (15 to 20 mm up), replaces with nothing:
    // cleared.
    let (empty_low, empty_high) = (on_canvas(&h, 10.0, 15.0), on_canvas(&h, 30.0, 20.0));
    box_drag(&mut h, empty_low, empty_high, Modifiers::NONE);
    assert!(h.state().tabs()[0].selection().is_empty());
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some("Window selection: 0 edges")
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_drag_starting_on_geometry_selects_it_and_boxes_nothing() {
    use eframe::egui::Modifiers;
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = box_from_the_front(&f);
    // Pressed on the front face, Face3 under the press: dragged across the whole box, the
    // face is selected, and none of the edges a box there would take.
    let from = on_canvas(&h, 20.0, 5.0);
    answering(&record, Some((1, Face(3), pixel_at(&h, from))), None);
    let to = on_canvas(&h, 45.0, -5.0);
    box_drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Face3"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_drag_whose_press_the_hover_hasnt_answered_waits_for_it_then_boxes() {
    use eframe::egui::Modifiers;
    let f = fixture();
    let (mut h, record) = box_from_the_front(&f);
    // The renderer holds its answers: the press's pixel isn't answered through the drag,
    // so nothing is decided.
    let (low, high) = (on_canvas(&h, -2.0, -2.0), on_canvas(&h, 42.0, 2.0));
    record.lock().unwrap().hold = true;
    box_press(&mut h, low, high, Modifiers::NONE);
    for _ in 0..30 {
        step(&mut h);
    }
    assert!(selected_names(&h).is_empty());
    // Let go still unanswered: it waits, its corners kept, as a click waits.
    box_release(&mut h, high, Modifiers::NONE);
    for _ in 0..30 {
        step(&mut h);
    }
    assert!(selected_names(&h).is_empty());
    // Answered (nothing under the press): the box it was, taking the window's edges.
    record.lock().unwrap().hold = false;
    step_until(&mut h, "the box decided", |h| {
        selected_names(h) == box_edges(&[1, 5, 9, 10])
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_box_cancelled_by_the_focus_going_selects_nothing() {
    use eframe::egui::{Event, Modifiers};
    let f = fixture();
    let (mut h, _record) = box_from_the_front(&f);
    let (low, high) = (on_canvas(&h, -2.0, -2.0), on_canvas(&h, 42.0, 2.0));
    box_drag(&mut h, low, high, Modifiers::NONE);
    let before = selected_names(&h);
    assert_eq!(before, box_edges(&[1, 5, 9, 10]));
    // A crossing dragged, then the window loses the focus: a Cancel, and nothing changes.
    let (right_low, left_high) = (on_canvas(&h, 42.0, -2.0), on_canvas(&h, -2.0, 12.0));
    box_press(&mut h, right_low, left_high, Modifiers::NONE);
    h.event(Event::WindowFocused(false));
    step(&mut h);
    h.event(Event::WindowFocused(true));
    box_release(&mut h, left_high, Modifiers::NONE);
    assert_eq!(selected_names(&h), before);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn with_the_edge_filter_off_a_box_takes_nothing_and_says_so_and_the_others_leave_it_be() {
    use eframe::egui::{Key, Modifiers};
    let f = fixture();
    let (mut h, _record) = box_from_the_front(&f);
    let (low, high) = (on_canvas(&h, -2.0, -2.0), on_canvas(&h, 42.0, 12.0));
    let every: Vec<String> = box_edges(&(1..=12).collect::<Vec<_>>());
    // X and V off (faces and vertices): a box in a part takes edges all the same, as the
    // prototype's `onBox` always does.
    h.key_press(Key::X);
    h.key_press(Key::V);
    step(&mut h);
    let filter = h.state().tabs()[0].selection().filter;
    assert_eq!(
        (filter.faces, filter.edges, filter.vertices),
        (false, true, false)
    );
    box_drag(&mut h, low, high, Modifiers::NONE);
    assert_eq!(selected_names(&h), every);
    // E: edges off too. A box takes nothing, the selection is kept, and the toast says why
    // (the prototype's words).
    h.key_press(Key::E);
    step(&mut h);
    let (inner_low, inner_high) = (on_canvas(&h, -2.0, -2.0), on_canvas(&h, 42.0, 2.0));
    box_drag(&mut h, inner_low, inner_high, Modifiers::NONE);
    assert_eq!(selected_names(&h), every);
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some("The edge filter is off, and a box selects edges in a part.")
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_flick_of_a_box_while_the_pointer_moves_every_frame_is_decided_and_selects() {
    use eframe::egui::{Modifiers, PointerButton};
    let f = fixture();
    let (mut h, _record) = box_from_the_front(&f);
    // The pointer arrives and presses in one frame, moves every frame after, and lets go in
    // the frame after its last move: no frame at rest for the hover to answer the press's
    // pixel before the drag goes on.
    let (low, high) = (on_canvas(&h, -2.0, -2.0), on_canvas(&h, 42.0, 2.0));
    h.hover_at(low);
    button_event(&h, low, PointerButton::Primary, true, Modifiers::NONE);
    step(&mut h);
    for i in 1..=3u8 {
        h.hover_at(low + (high - low) * f32::from(i) / 3.0);
        step(&mut h);
    }
    button_event(&h, high, PointerButton::Primary, false, Modifiers::NONE);
    h.hover_at(high + eframe::egui::vec2(5.0, 5.0));
    step(&mut h);
    for i in 1..=4u8 {
        h.hover_at(high + eframe::egui::vec2(5.0 + f32::from(i), 5.0));
        step(&mut h);
    }
    step_until(&mut h, "the box decided", |h| {
        selected_names(h) == box_edges(&[1, 5, 9, 10])
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn x_e_and_v_typed_into_a_field_leave_the_filter_be() {
    use eframe::egui::Key;
    let f = fixture();
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES);
    meshes_settled(&mut h);
    step_until(&mut h, "the browser's bodies", |h| {
        shows(h, "Browser", "Large")
    });
    browser(&h).get_by_label("Large").click();
    step_until(&mut h, "Large selected", |h| {
        toggled(&browser(h).get_by_label("Large")) == Some(true)
    });
    name_field(&h).focus();
    step(&mut h);
    for key in [Key::X, Key::E, Key::V] {
        h.key_press(key);
        step(&mut h);
    }
    step(&mut h);
    assert_eq!(
        h.state().tabs()[0].selection().filter,
        silico_app::selection::Filter::default()
    );
    // F5 in the field is the field's too: no toast.
    h.key_press(Key::F5);
    step(&mut h);
    assert_eq!(h.state().toast(), None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn f5_says_where_the_filters_are_in_a_toast_for_its_time() {
    use eframe::egui::Key;
    // F5: the prototype's toast, over the canvas, and nothing else changed; gone after its
    // 3.8 seconds. Not on the shortcut sheet, as the prototype's isn't.
    let f = fixture();
    let (mut h, _record) = f.drawn("example.silico", EXAMPLE);
    meshes_settled(&mut h);
    let time = |h: &AppHarness| h.ctx.input(|input| input.time);
    // A frame a sixtieth of a second, the harness's.
    let frame = 1.0 / 60.0;
    assert_eq!(h.state().toast(), None);
    h.key_press(Key::F5);
    step(&mut h);
    step(&mut h);
    let (said, shown) = h.state().toast().unwrap();
    assert_eq!(said, silico_app::app::FILTERS_SAID);
    // Over the canvas's bottom right, 12 in from its right and 58 up from its foot, its
    // text inside its padding (8 at the right, 7 below) and border.
    let canvas = the_viewport(&h);
    let text = h.get_by_label(silico_app::app::FILTERS_SAID).rect();
    let right = canvas.right() - 12.0 - 8.0 - 1.0;
    let foot = canvas.bottom() - 58.0 - 7.0 - 1.0;
    assert!(
        (text.right() - right).abs() <= 1.0,
        "{text:?} in {canvas:?}"
    );
    assert!(
        (text.bottom() - foot).abs() <= 2.0,
        "{text:?} in {canvas:?}"
    );
    assert_eq!(
        h.state().tabs()[0].selection().filter,
        silico_app::selection::Filter::default()
    );
    // Up for 3.8 seconds by egui's clock, taken down in the first frame after.
    let mut frames = 0;
    while h.state().toast().is_some() {
        assert!(time(&h) - shown < 3.8, "up past its time");
        step(&mut h);
        frames += 1;
        assert!(frames < 100_000, "never taken down");
    }
    let up = time(&h) - shown;
    assert!((3.8..3.8 + frame).contains(&up), "{up}");
    step(&mut h);
    assert!(!anywhere(&h, silico_app::app::FILTERS_SAID));
    assert!(
        !REGISTRY.iter().flat_map(|entry| entry.keys).any(
            |chord| matches!(chord, Chord::Key(k) | Chord::Exact(k) if k.logical_key == Key::F5)
        )
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_drag_on_geometry_picks_nothing_once_another_model_is_published_or_the_filter_changes() {
    use eframe::egui::{Key, Modifiers, PointerButton};
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = box_from_the_front(&f);
    let from = on_canvas(&h, 20.0, 5.0);
    let to = on_canvas(&h, 45.0, -5.0);
    answering(&record, Some((1, Face(3), pixel_at(&h, from))), None);
    let drag_after = |h: &mut AppHarness, change: &dyn Fn(&mut AppHarness)| {
        // Pressed on Face3, its hover answered; then the change, then the drag on.
        h.hover_at(from);
        for _ in 0..3 {
            step(h);
        }
        button_event(h, from, PointerButton::Primary, true, Modifiers::NONE);
        step(h);
        change(h);
        box_drag_on(h, from, to);
    };
    // Another model published, its meshes not read yet: Face3's name was read from the
    // model before's, so nothing is picked, and the toast says why.
    let hold = f
        .kernel
        .hold(|op| matches!(op, silico_core::kernel::Op::Tessellate { .. }));
    drag_after(&mut h, &|h| {
        published_with_meshes_held(&f, h, "length", "50")
    });
    assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some(silico_app::box_select::PRESS_MOVED)
    );
    hold.release();
    step_until(&mut h, "the new model's meshes", meshes_current);
    // The filter changed under the press (V): let go too.
    answering(&record, Some((1, Face(3), pixel_at(&h, from))), None);
    drag_after(&mut h, &|h| {
        h.key_press(Key::V);
        step(h);
    });
    assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some(silico_app::box_select::PRESS_MOVED)
    );
    assert_eq!(never_waits::waits(), 0);
}

/// A press already down at `from` dragged to `to` in four frames and let go.
fn box_drag_on(h: &mut AppHarness, from: eframe::egui::Pos2, to: eframe::egui::Pos2) {
    use eframe::egui::{Modifiers, PointerButton};
    for i in 1..=4u8 {
        h.hover_at(from + (to - from) * f32::from(i) / 4.0);
        step(h);
    }
    button_event(h, to, PointerButton::Primary, false, Modifiers::NONE);
    step(h);
    step(h);
}

#[test]
fn a_press_on_a_body_hidden_since_its_hover_answered_picks_none_of_its_faces() {
    use eframe::egui::{Key, Modifiers};
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = box_from_the_front(&f);
    step_until(&mut h, "the browser's body", |h| {
        shows(h, "Browser", "Block")
    });
    browser(&h).get_by_label("Block").click();
    step_until(&mut h, "Block selected", |h| {
        toggled(&browser(h).get_by_label("Block")) == Some(true)
    });
    // The hover answers Face3 at P; then H hides the body, the pointer still: the renderer
    // finds nothing at P now, and the hover isn't asked again (the pointer didn't move).
    let at = on_canvas(&h, 20.0, 5.0);
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    h.hover_at(at);
    step_until(&mut h, "the hover", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
    answering(&record, None, None);
    h.key_press(Key::H);
    step(&mut h);
    step(&mut h);
    assert!(!h.state().tabs()[0].hidden.is_empty());
    // A press-drag at P: not the hidden face; a box, which takes nothing hidden.
    let to = on_canvas(&h, 45.0, -5.0);
    box_drag(&mut h, at, to, Modifiers::NONE);
    assert!(
        !selected_names(&h).iter().any(|name| name.contains("Face")),
        "{:?}",
        selected_names(&h)
    );
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some("Window selection: 0 edges")
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_press_after_the_camera_moved_under_a_still_pointer_isnt_decided_by_the_hover_before() {
    use eframe::egui::Modifiers;
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = box_from_the_front(&f);
    // The hover answers Face3 at P; then the camera moves (a view key's), the pointer
    // still, so P shows nothing now and the hover isn't asked again.
    let at = on_canvas(&h, 20.0, 5.0);
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    h.hover_at(at);
    step_until(&mut h, "the hover", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
    answering(&record, None, None);
    let mut camera = view(&h).camera().clone();
    camera.eye[2] += 30.0;
    camera.target[2] += 30.0;
    view_mut(&mut h).set_camera(camera);
    step(&mut h);
    step(&mut h);
    // A press-drag at P: decided by an answer of the moved view (nothing there), a box,
    // never Face3.
    let to = at + eframe::egui::vec2(60.0, 40.0);
    box_drag(&mut h, at, to, Modifiers::NONE);
    assert!(
        !selected_names(&h).iter().any(|name| name.contains("Face")),
        "{:?}",
        selected_names(&h)
    );
    assert!(
        h.state()
            .toast()
            .is_some_and(|(said, _)| said.starts_with("Window selection: ")),
        "{:?}",
        h.state().toast()
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- The fillet dialog (M5.8): its edges from the canvas selection ----

/// MODIFY's `kind` clicked in its row, and its dialog open.
fn modify(h: &mut AppHarness, kind: &str) {
    h.get_by_role_and_label(Role::Toolbar, "MODIFY")
        .get_by_label(kind)
        .click();
    step_until(h, "the dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, kind).is_some()
    });
}

/// Edge `n` of body `body` found under the canvas's centre from now on, and clicked there,
/// Ctrl held if `ctrl`.
fn click_edge(h: &mut AppHarness, record: &Record, body: u32, n: u32, ctrl: bool) {
    use silico_app::renderer::Element::Edge;
    let at = the_viewport(h).center();
    let centre = pixel_at(h, at);
    answering(record, None, Some((body, Edge(n), centre)));
    click_canvas(h, at, ctrl);
}

/// The names the open dialog's box of names holds.
fn boxed(h: &AppHarness) -> Vec<String> {
    h.state().tabs()[0]
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.names())
        .map(|names| names.names.clone())
        .unwrap_or_default()
}

#[track_caller]
fn boxed_until(h: &mut AppHarness, want: &[&str]) {
    step_until(h, &format!("{want:?} in the box"), |h| boxed(h) == want);
}

/// What the open dialog's box of names says under it, if anything.
fn box_said(h: &AppHarness) -> Option<String> {
    h.state().tabs()[0]
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.names())
        .and_then(|names| names.said.as_ref())
        .map(|said| said.text().to_owned())
}

/// The open dialog's box of names' body, the feature's `base`, as it holds it.
fn box_body(h: &AppHarness) -> Option<String> {
    h.state().tabs()[0]
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.names())
        .and_then(|names| names.body.as_ref())
        .map(ToString::to_string)
}

/// The line of `text` that holds the feature of type `kind`, as the document writes it,
/// without its indent and its comma.
fn feature_line(text: &str, kind: &str) -> String {
    text.lines()
        .find(|line| line.contains(&format!(r#""type": "{kind}""#)))
        .unwrap()
        .trim()
        .trim_end_matches(',')
        .to_owned()
}

/// `text` read as JSON.
fn plain(text: &str) -> Plain {
    Plain::of(&json::read(text.as_bytes()).unwrap())
}

/// The field `field` of the feature named `name`, as the session's document holds it.
fn written(f: &Fixture, h: &AppHarness, name: &str, field: &str) -> Option<Plain> {
    fields_of(&text_of(f, h), name)?
        .into_iter()
        .find(|(key, _)| key == field)
        .map(|(_, value)| value)
}

/// The ID the open dialog's feature was given.
fn dialog_feature(h: &AppHarness) -> String {
    h.state().tabs()[0]
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.feature.as_ref())
        .unwrap()
        .to_string()
}

#[test]
fn a_fillet_dialog_fed_the_selection_of_two_edges_writes_a_fillet_naming_them() {
    // M5.8's done test. Two of the box's edges selected in the canvas, then MODIFY's Fillet:
    // its box takes them, listed by name, and its radius is the first, 1 mm (Silico's own
    // choice: nothing was OK'd before); the feature it adds is the line below, written by
    // hand, its ID the core's.
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let before = text_of(&f, &h);
    click_edge(&mut h, &record, 1, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    click_edge(&mut h, &record, 1, 5, true);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    // Listed: the count in the box, each name under it with its remove button.
    assert!(dialog(&h).query_by_label("2 edges").is_some());
    for name in ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"] {
        assert!(dialog(&h).query_by_label(name).is_some(), "{name}");
        assert!(
            dialog(&h)
                .query_by_role_and_label(Role::Button, &format!("Remove {name}"))
                .is_some(),
            "{name}"
        );
    }
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("1"));
    // The selection stays, the edges highlighted.
    assert_eq!(
        selected_names(&h),
        ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]
    );
    let id = dialog_feature(&h);
    assert!(ok_enabled(&h));
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    let text = text_of(&f, &h);
    // The feature as its JSON line, written by hand, but its ID and version, the core's.
    assert_eq!(
        fields_of(&text, "Fillet"),
        Some(expected(
            r#"{"type": "fillet", "name": "Fillet", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"], "radius": 1}"#
        ))
    );
    // And the document's text, byte for byte, as the canonical writer lays it out (a line
    // too long for one line is broken member by member), the box as it was.
    let want = format!(
        r#"{{
  "silico": 1,
  "units": {{"length": "mm", "resolution": "1nm"}},
  "naming": 5,
  "features": [
    {{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}},
    {{
      "id": "{id}",
      "type": "fillet",
      "v": 1,
      "name": "Fillet",
      "base": "k7f2qx3m",
      "edges": [
        "k7f2qx3m.box.Edge3",
        "k7f2qx3m.box.Edge5"
      ],
      "radius": 1
    }}
  ]
}}
"#
    );
    assert_eq!(text, want);
    assert!(before.contains(feature_line(&text, "box").as_str()));
    assert_eq!(h.state().tabs()[0].sent, [Sent::Begin, Sent::Add]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn in_the_fillet_dialog_a_ctrl_click_adds_an_edge_and_a_click_on_a_listed_edge_removes_it() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    click_edge(&mut h, &record, 1, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    click_edge(&mut h, &record, 1, 5, true);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    // Ctrl-click a third: added to the box and the selection, and written.
    click_edge(&mut h, &record, 1, 7, true);
    boxed_until(
        &mut h,
        &[
            "k7f2qx3m.box.Edge3",
            "k7f2qx3m.box.Edge5",
            "k7f2qx3m.box.Edge7",
        ],
    );
    settled(&mut h);
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(
            r#"["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5", "k7f2qx3m.box.Edge7"]"#
        ))
    );
    assert!(dialog(&h).query_by_label("3 edges").is_some());
    // A plain click on a listed edge: removed from both, as SolidWorks' box takes a click.
    click_edge(&mut h, &record, 1, 5, false);
    boxed_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge7"]);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge7"]);
    settled(&mut h);
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(r#"["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge7"]"#))
    );
    // A face under the pointer, no edge: the box takes edges alone, so nothing is picked
    // (its pick answered before the renderer's answer changes: nothing, not even a face
    // the box would refuse, said); nor does a click on nothing clear the box.
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    for _ in 0..4 {
        step(&mut h);
    }
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge7"]);
    assert_eq!(box_said(&h), None);
    answering(&record, None, None);
    click_canvas(&mut h, at, false);
    for _ in 0..6 {
        step(&mut h);
    }
    assert_eq!(box_said(&h), None);
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge7"]);
    assert_eq!(
        selected_names(&h),
        ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge7"]
    );
    // A listed edge's remove button: let go of from the box and the selection.
    dialog(&h)
        .get_by_role_and_label(Role::Button, "Remove k7f2qx3m.box.Edge3")
        .click();
    boxed_until(&mut h, &["k7f2qx3m.box.Edge7"]);
    selected_until(&mut h, &["k7f2qx3m.box.Edge7"]);
    settled(&mut h);
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(r#"["k7f2qx3m.box.Edge7"]"#))
    );
    // Closed, the canvas picks as before: a plain click replaces the selection.
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    click_edge(&mut h, &record, 1, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn in_the_fillet_dialog_a_box_over_faces_and_edges_toggles_only_the_edges_into_its_box() {
    use eframe::egui::Modifiers;
    use silico_app::renderer::Element::Edge;
    let f = fixture();
    let (mut h, record) = box_from_the_front(&f);
    click_edge(&mut h, &record, 1, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3"]);
    // Nothing under the pointer: a plain window from (-2, -2) to (42, 12) mm, round the
    // whole part, its faces and all 12 edges inside. The fillet's box takes edges, so the
    // box takes the edges alone, and toggles each, as a click in the box does: Edge3, in
    // the box already, let go; the other 11 added after it, by number, as the box takes
    // them. A Replace would have kept Edge3 and told the dialog nothing.
    answering(&record, None, None);
    let (low, high) = (on_canvas(&h, -2.0, -2.0), on_canvas(&h, 42.0, 12.0));
    box_drag(&mut h, low, high, Modifiers::NONE);
    let toggled = box_edges(&[1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    let want: Vec<&str> = toggled.iter().map(String::as_str).collect();
    boxed_until(&mut h, &want);
    selected_until(&mut h, &want);
    assert_eq!(
        h.state().toast().map(|(said, _)| said),
        Some("Window selection: 12 edges")
    );
    assert_eq!(box_said(&h), None);
    settled(&mut h);
    let written_edges = format!(
        "[{}]",
        want.iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(&written_edges))
    );
    // Shift held, the same window again: toggled, not added, so all 11 let go and Edge3
    // back, alone.
    box_drag(&mut h, low, high, Modifiers::SHIFT);
    boxed_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    // A plain drag pressed on Edge5 picks it as a click does, toggled into the box: added
    // after Edge3, not replacing it.
    let from = on_canvas(&h, 20.0, 0.0);
    answering(&record, None, Some((1, Edge(5), pixel_at(&h, from))));
    let to = on_canvas(&h, 45.0, -5.0);
    box_drag(&mut h, from, to, Modifiers::NONE);
    boxed_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_fillet_dialog_refuses_an_unstable_selected_name_saying_why_and_writes_nothing() {
    let f = fixture();
    f.kernel.answer_instead(|op| match op {
        Op::Boolean {
            op: silico_core::kernel::BooleanOp::Fuse,
            base,
            ..
        } => Some(silico_protocol::Reply::Built(
            silico_core::kernel::BuiltData {
                // A box a mm³ larger than the base, so the fuse changes it (S7's rule).
                shape: FakeKernel::resized(base, 1.0)?,
                history: numbering_fuse(),
            },
        )),
        _ => None,
    });
    let (mut h, record) = f.drawn("fused.silico", FUSED);
    meshes_settled(&mut h);
    let body = body_of(&h, "rrrrrrrr");
    let before = text_of(&f, &h);
    // The fuse's Edge1, which the fuse's history doesn't report (the base's first edge
    // isn't kept, the tool's none): named by its number, unstable, and selectable.
    click_edge(&mut h, &record, body, 1, false);
    selected_until(&mut h, &["rrrrrrrr.fuse.Edge1"]);
    // Refused as a fillet's reference refuses it, in references.rs's words (N3, by hand),
    // at its place in the selection; and its suggestion after it, in brackets, as the tab
    // says a refusal: the whole line written by hand.
    let why = "selection.0, \"rrrrrrrr.fuse.Edge1\", names an edge, whose name is unstable (it \
               came about as another element did, or is its number): an edit could make it \
               another element";
    let refusal = h.state().tabs()[0].selection().storable().unwrap_err();
    assert_eq!(refusal.message(), why);
    let said = "selection.0, \"rrrrrrrr.fuse.Edge1\", names an edge, whose name is unstable (it \
                came about as another element did, or is its number): an edit could make it \
                another element (name the edge as its feature now names it: the candidates \
                are the likeliest, none taken for it)";
    modify(&mut h, "Fillet");
    step_until(&mut h, "the refusal in the dialog", |h| {
        dialog(h).query_by_label(said).is_some()
    });
    // Nothing taken, nothing written; the selection let go, as the box holds nothing.
    for _ in 0..6 {
        step(&mut h);
    }
    assert!(boxed(&h).is_empty());
    assert!(!ok_enabled(&h));
    assert!(
        dialog(&h)
            .query_by_label("Select edges in the canvas")
            .is_some()
    );
    assert!(h.state().tabs()[0].selection().is_empty());
    assert_eq!(h.state().tabs()[0].sent, []);
    assert_eq!(text_of(&f, &h), before);
    // Clicked again with the dialog open: refused the same way, at its place in the pick,
    // and let go from the selection again.
    click_edge(&mut h, &record, body, 1, true);
    step_until(&mut h, "the refusal in the dialog", |h| {
        dialog(h).query_by_label(said).is_some()
    });
    for _ in 0..6 {
        step(&mut h);
    }
    assert!(boxed(&h).is_empty());
    assert!(h.state().tabs()[0].selection().is_empty());
    assert_eq!(h.state().tabs()[0].sent, []);
    assert_eq!(text_of(&f, &h), before);
    // A stable edge, the base's Edge2, kept by the fuse: taken, the refusal gone.
    click_edge(&mut h, &record, body, 2, true);
    boxed_until(&mut h, &["aaaaaaaa.box.Edge2"]);
    assert!(dialog(&h).query_by_label(said).is_none());
    added(&mut h);
    settled(&mut h);
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(r#"["aaaaaaaa.box.Edge2"]"#))
    );
    assert_eq!(
        written(&f, &h, "Fillet", "base"),
        Some(plain(r#""rrrrrrrr""#))
    );
    assert_eq!(never_waits::waits(), 0);
}

/// A fillet's history, as OCCT's consumes the edges it rounds: its base, a box, is the
/// result, each face, edge, and vertex kept by its own number but the edges `rounded`,
/// which nothing reports (the result's of those numbers are named by their numbers).
fn consuming_fillet(rounded: &[u32]) -> silico_core::kernel::History {
    use silico_core::kernel::{ElementCounts, ElementFate, History, InputHistory, Topology};
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    History {
        inputs: vec![InputHistory {
            faces: (1..=6).map(kept).collect(),
            edges: (1..=12)
                .map(|n| {
                    if rounded.contains(&n) {
                        ElementFate::default()
                    } else {
                        kept(n)
                    }
                })
                .collect(),
            vertices: (1..=8).map(kept).collect(),
        }],
        result: ElementCounts {
            faces: 6,
            edges: 12,
            vertices: 8,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; 6],
            outer_edges: vec![vec![]; 6],
            edge_vertices: vec![vec![]; 12],
            solid_faces: vec![(1..=6).collect()],
        },
    }
}

/// The fake kernel's fillets of a box consume the edges they round, as OCCT's do.
fn fillets_consume_their_edges(f: &Fixture) {
    f.kernel.answer_instead(|op| match op {
        Op::Fillet { shape, edges, .. } => Some(silico_protocol::Reply::Built(
            silico_core::kernel::BuiltData {
                shape: shape.clone(),
                history: consuming_fillet(edges),
            },
        )),
        _ => None,
    });
}

#[test]
fn a_fillet_dialogs_cancel_after_earlier_unsaved_edits_leaves_the_text_as_before_it() {
    let f = fixture();
    meshes_a_real_box(&f);
    fillets_consume_their_edges(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    // An edit before the dialog, in the transaction: the box renamed.
    select(&mut h, "Block");
    type_name(&mut h, "K");
    settled(&mut h);
    let before = text_of(&f, &h);
    assert!(before.contains(r#""name": "BlockK""#));
    let opened_at = h.state().tabs()[0].revision;
    click_edge(&mut h, &record, 1, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    // Its radius typed (a 2 after the 1: 12), and a second edge taken: both written.
    type_into(&mut h, "Radius", "2");
    click_edge(&mut h, &record, 1, 5, true);
    boxed_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    settled(&mut h);
    assert_eq!(written(&f, &h, "Fillet", "radius"), Some(plain("12")));
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(r#"["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]"#))
    );
    // The preview consumed both edges: gone from the selection, unsaid, the box holding
    // them.
    step_until(&mut h, "the consumed edges let go", |h| {
        selected_names(h).is_empty()
    });
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
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
    // The selection is as it was before the dialog opened, as SolidWorks returns to the
    // selection its command started with: Edge3 alone (Edge5 was picked into the box), found
    // again in the box the Cancel's undo gave back, nothing said of it.
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    for _ in 0..6 {
        step(&mut h);
    }
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Edge3"]);
    assert!(
        h.state().tabs()[0]
            .selection()
            .elements()
            .all(|p| p.stable())
    );
    assert_eq!(h.state().tabs()[0].selection().said(), None);
    // Read again in the model the undo published, not held as put back: drawn on its
    // meshes.
    meshes_settled(&mut h);
    step(&mut h);
    let block = body_of(&h, "k7f2qx3m");
    assert_eq!(
        highlighted(&record),
        [(block, silico_app::renderer::Element::Edge(3))]
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_fillet_too_large_shows_the_kernels_refusal_naming_its_edge_and_the_rest_still_draws() {
    // The fake kernel refuses a fillet past 5 mm as OCCT refuses one it can't build, naming
    // the edges it was asked for (`Refusal::EdgesFailed`).
    let f = fixture();
    meshes_a_real_box(&f);
    f.kernel.refuse_as_worker(|op| match op {
        Op::Fillet { edges, radius, .. } if *radius > 5.0 => Some(silico_protocol::Failure::new(
            silico_protocol::Refusal::EdgesFailed {
                dressing: silico_protocol::DressingKind::Fillet,
                edges: edges.clone(),
            },
        )),
        _ => None,
    });
    let (mut h, record) = f.drawn("blocks.silico", TWO_BLOCKS);
    meshes_settled(&mut h);
    let block = body_of(&h, "k7f2qx3m");
    click_edge(&mut h, &record, block, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    // 1 mm builds.
    assert_eq!(h.state().tabs()[0].errors(), 0);
    // 13 mm (a 3 after the 1) doesn't: the kernel's refusal in the dialog, as the core
    // gives it, its message, its cause naming the edge, the edge among its elements, and
    // its suggestion (refusal.rs's and worker.rs's words, by hand). The fake kernel's
    // errors name an edge by its number in the base (`Edge3`), as FreeCAD does; a worker's
    // name it as the document does (`KernelRequest::with_edge_names`).
    type_into(&mut h, "Radius", "3");
    let lines = [
        "Fillet failed",
        "OCCT couldn't fillet Edge3",
        "Elements: Edge3",
        "if the fillet runs past a face beside the edge, try a smaller radius; if not, try \
         another radius, or leave the edge out: OCCT can't fillet some edges at any size",
    ];
    step_until(&mut h, "the refusal in the dialog", |h| {
        lines
            .iter()
            .all(|line| dialog(h).query_by_label(line).is_some())
    });
    assert_eq!(h.state().tabs()[0].errors(), 1);
    // The rest of the model still draws: the other block's body, read with its meshes.
    meshes_settled(&mut h);
    step(&mut h);
    let other = body_of(&h, "a91cw0de");
    let drawn = record.lock().unwrap().frames.last().unwrap().bodies.clone();
    assert!(drawn.contains(&other), "{drawn:?}");
    // OK keeps it with its error, as SolidWorks saves a part with rebuild errors.
    assert!(ok_enabled(&h));
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(written(&f, &h, "Fillet", "radius"), Some(plain("13")));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn on_lavapipe_the_fillet_dialog_lists_two_edges_picked_in_the_canvas_and_they_stay_lit() {
    on_lavapipe(
        "on_lavapipe_the_fillet_dialog_lists_two_edges_picked_in_the_canvas_and_they_stay_lit",
        || {
            use silico_app::renderer::Element::Edge;
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
            h.set_render_every_step(true);
            silico_app::theme::apply(&h.ctx);
            let path = f.file("box.silico", A_BOX);
            h.state_mut().open(path.to_str().unwrap(), false);
            meshes_settled(&mut h);
            // From the front, as the selection's lavapipe test looks: a point (x, 0, z) of
            // the front face is seen (x − 20) mm right of the pane's centre and (z − 5) mm
            // above it, a mm being the pane's height over 20.
            view_mut(&mut h).set_camera(silico_protocol::Camera {
                projection: silico_protocol::Projection::Orthographic { view_height: 20.0 },
                eye: [20.0, -100.0, 5.0],
                target: [20.0, 10.0, 5.0],
                up: [0.0, 0.0, 1.0],
            });
            meshes_settled(&mut h);
            for _ in 0..3 {
                step(&mut h);
            }
            let rect = the_viewport(&h);
            let mm = rect.height() / 20.0;
            let seen = |x: f32, z: f32| {
                rect.center() + eframe::egui::vec2((x - 20.0) * mm, -(z - 5.0) * mm)
            };
            let select_at = |h: &mut AppHarness, at, ctrl, want: &[&str]| {
                click_canvas(h, at, ctrl);
                for _ in 0..12 {
                    if selected_names(h) == want {
                        break;
                    }
                    step(h);
                }
                assert_eq!(selected_names(h), want, "at {at:?}");
            };
            // The front face's top edge (z = 10) from 2 points below its line, and, Ctrl
            // held, its bottom edge (z = 0) from 2 points above: inside the 3-point band
            // of each, so each edge, not the face. Edge3 and Edge1 in the fixture's
            // numbering (`silico_render::bench::box_mesh`: the front face's bottom, right,
            // top, left first).
            select_at(
                &mut h,
                seen(30.0, 10.0) + eframe::egui::vec2(0.0, 2.0),
                false,
                &["k7f2qx3m.box.Edge3"],
            );
            select_at(
                &mut h,
                seen(30.0, 0.0) - eframe::egui::vec2(0.0, 2.0),
                true,
                &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge1"],
            );
            modify(&mut h, "Fillet");
            added(&mut h);
            settled(&mut h);
            meshes_settled(&mut h);
            for _ in 0..3 {
                step(&mut h);
            }
            // Listed in the box, and lit on the preview's body (the fake keeps every
            // element of the box, so both edges are there by their names).
            assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge1"]);
            assert!(dialog(&h).query_by_label("2 edges").is_some());
            let lit: Vec<silico_app::renderer::Element> = view(&h)
                .highlights
                .iter()
                .map(|(_, element)| *element)
                .collect();
            assert_eq!(lit, [Edge(1), Edge(3)]);
            if let Some(shot) = std::env::var_os("SILICO_APP_FILLET_SHOT") {
                h.render().unwrap().save(shot).unwrap();
            }
            assert_eq!(never_waits::waits(), 0);
        },
    );
}

/// The dialog's input `label` emptied of `old` (the cursor at its end, Backspace takes one
/// character each) and `new` typed in, one keystroke a frame.
fn retype(h: &mut AppHarness, label: &str, old: &str, new: &str) {
    input(h, label).focus();
    step(h);
    for _ in 0..old.len() {
        h.key_press(eframe::egui::Key::Backspace);
        step(h);
    }
    input(h, label).type_text(new);
    step(h);
    assert_eq!(input(h, label).value().as_deref(), Some(new));
}

/// Edit Feature on the timeline's `name`, from its context menu.
fn edit_from_the_timeline(h: &mut AppHarness, name: &str) {
    h.get_by_role_and_label(Role::Pane, "Timeline")
        .get_by_label(name)
        .click_secondary();
    step(h);
    step(h);
    h.get_by_label("Edit Feature").click();
}

/// The dialog's Cancel clicked, and the dialog closed.
fn cancel_dialog(h: &mut AppHarness) {
    dialog(h)
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    step_until(h, "the dialog closed", |h| !dialog_open(h));
    settled(h);
}

const BOX_ROUND: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "radius": 0.5}
  ]
}
"#;

#[test]
fn a_fillets_radius_opens_at_the_last_okd_cancel_leaves_it_and_edit_feature_shows_its_own() {
    // SolidWorks' Fillet PropertyManager "remembers its last used state" (docs/ux/
    // solidworks.md, §5): the first fillet at Silico's first 1 mm, OK'd at 2.5; the next
    // opens at 2.5; one cancelled at 7 leaves 2.5; Edit Feature on Round shows its own 0.5;
    // a chamfer opens at its own first value, 1.
    let f = fixture();
    let (mut h, record) = f.drawn("round.silico", BOX_ROUND);
    meshes_settled(&mut h);
    let body = body_of(&h, "f0000001");
    click_edge(&mut h, &record, body, 5, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge5"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("1"));
    retype(&mut h, "Radius", "1", "2.5");
    settled(&mut h);
    assert_eq!(written(&f, &h, "Fillet", "radius"), Some(plain("2.5")));
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    // Opened again: at 2.5, the value OK'd.
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("2.5"));
    retype(&mut h, "Radius", "2.5", "7");
    settled(&mut h);
    cancel_dialog(&mut h);
    // Cancelled at 7: still 2.5.
    modify(&mut h, "Fillet");
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("2.5"));
    cancel_dialog(&mut h);
    // Edit Feature: the feature's own radius, not the one remembered.
    edit_from_the_timeline(&mut h, "Round");
    step_until(&mut h, "Round's dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Fillet").is_some()
    });
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("0.5"));
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3"]);
    cancel_dialog(&mut h);
    // A chamfer remembers its own: none OK'd yet, so its first, 1.
    modify(&mut h, "Chamfer");
    assert_eq!(input(&h, "Distance").value().as_deref(), Some("1"));
    cancel_dialog(&mut h);
    // And the fillet's is kept still.
    modify(&mut h, "Fillet");
    assert_eq!(input(&h, "Radius").value().as_deref(), Some("2.5"));
    cancel_dialog(&mut h);
    assert_eq!(never_waits::waits(), 0);
}

const MID_CHAIN: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "radius": 1},
    {"id": "a91cw0de", "type": "box", "v": 1, "name": "Tool", "position": [80, 0, 0], "length": 10, "width": 10, "height": 10},
    {"id": "r0000001", "type": "fuse", "v": 1, "name": "Joined", "base": "f0000001", "tool": "a91cw0de"}
  ]
}
"#;

#[test]
fn edit_feature_on_a_fillet_a_later_feature_builds_on_takes_its_bases_edges_from_the_body() {
    // Round fillets the box's Edge3, and Joined fuses Round with Tool: the body is Joined's,
    // and the canvas draws it. Round's dialog reads a pick there against its base, the box,
    // as SolidWorks reads a feature's picks on the model rolled back to before it.
    let f = fixture();
    let (mut h, record) = f.drawn("chain.silico", MID_CHAIN);
    meshes_settled(&mut h);
    let body = body_of(&h, "r0000001");
    edit_from_the_timeline(&mut h, "Round");
    step_until(&mut h, "Round's dialog", |h| {
        h.query_by_role_and_label(Role::Dialog, "Fillet").is_some()
    });
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3"]);
    assert_eq!(box_body(&h).as_deref(), Some("k7f2qx3m"));
    // The box's Edge5, picked on Joined's body (kept by the fillet and the fuse): taken,
    // written, the base the box still.
    click_edge(&mut h, &record, body, 5, false);
    boxed_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    settled(&mut h);
    assert_eq!(box_said(&h), None);
    assert_eq!(
        written(&f, &h, "Round", "edges"),
        Some(plain(r#"["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]"#))
    );
    assert_eq!(
        written(&f, &h, "Round", "base"),
        Some(plain(r#""k7f2qx3m""#))
    );
    // Tool's Edge1, Joined's Edge13 (the fake's fuse numbers the tool's edges after the
    // base's 12): not on the box, so refused, said, and let go from the selection again.
    let after = "a91cw0de.box.Edge1 isn't on k7f2qx3m, which this fillet is built on: select \
                 one of k7f2qx3m's edges";
    click_edge(&mut h, &record, body, 13, true);
    step_until(&mut h, "the refusal in the dialog", |h| {
        dialog(h).query_by_label(after).is_some()
    });
    for _ in 0..6 {
        step(&mut h);
    }
    assert_eq!(box_said(&h).as_deref(), Some(after));
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Edge5"]);
    // Every name removed, then the box's Edge7 picked on Joined's body: taken, the base the
    // box still, nothing refused.
    for name in ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"] {
        dialog(&h)
            .get_by_role_and_label(Role::Button, &format!("Remove {name}"))
            .click();
        step_until(&mut h, "the name removed", |h| {
            !boxed(h).iter().any(|held| held == name)
        });
        // Drawn without it, so the next button is found where it's drawn now.
        step(&mut h);
    }
    assert!(boxed(&h).is_empty());
    click_edge(&mut h, &record, body, 7, true);
    boxed_until(&mut h, &["k7f2qx3m.box.Edge7"]);
    settled(&mut h);
    assert_eq!(box_body(&h).as_deref(), Some("k7f2qx3m"));
    assert_eq!(box_said(&h), None);
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(
        written(&f, &h, "Round", "edges"),
        Some(plain(r#"["k7f2qx3m.box.Edge7"]"#))
    );
    assert_eq!(
        written(&f, &h, "Round", "base"),
        Some(plain(r#""k7f2qx3m""#))
    );
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    assert_eq!(
        fields_of(&text_of(&f, &h), "Round"),
        Some(expected(
            r#"{"type": "fillet", "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge7"], "radius": 1}"#
        ))
    );
    assert_eq!(h.state().tabs()[0].errors(), 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_fillet_dialog_opened_on_a_face_and_an_edge_takes_the_edge_and_notes_the_face_left_out() {
    use silico_app::renderer::Element::Face;
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    let at = the_viewport(&h).center();
    answering(&record, Some((1, Face(3), pixel_at(&h, at))), None);
    click_canvas(&mut h, at, false);
    selected_until(&mut h, &["k7f2qx3m.box.Face3"]);
    click_edge(&mut h, &record, 1, 3, true);
    selected_until(&mut h, &["k7f2qx3m.box.Face3", "k7f2qx3m.box.Edge3"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    let note = "1 face left out: a fillet takes edges";
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3"]);
    assert_eq!(box_said(&h).as_deref(), Some(note));
    assert!(dialog(&h).query_by_label(note).is_some());
    assert_eq!(selected_names(&h), ["k7f2qx3m.box.Edge3"]);
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(r#"["k7f2qx3m.box.Edge3"]"#))
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn the_fillet_dialog_opened_on_two_bodies_edges_takes_none_says_why_and_clears_the_selection() {
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("blocks.silico", TWO_BLOCKS);
    meshes_settled(&mut h);
    let (block, other) = (body_of(&h, "k7f2qx3m"), body_of(&h, "a91cw0de"));
    click_edge(&mut h, &record, block, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    click_edge(&mut h, &record, other, 1, true);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3", "a91cw0de.box.Edge1"]);
    modify(&mut h, "Fillet");
    let why = "The selected edges are on 2 bodies, k7f2qx3m and a91cw0de: a fillet takes one \
               body's edges";
    step_until(&mut h, "the refusal in the dialog", |h| {
        dialog(h).query_by_label(why).is_some()
    });
    for _ in 0..6 {
        step(&mut h);
    }
    assert!(boxed(&h).is_empty());
    assert_eq!(box_said(&h).as_deref(), Some(why));
    assert!(h.state().tabs()[0].selection().is_empty());
    assert!(!ok_enabled(&h));
    assert_eq!(h.state().tabs()[0].sent, []);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_on_an_edge_the_box_lists_removes_it_though_the_timeline_replaced_the_selection() {
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    click_edge(&mut h, &record, 1, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    click_edge(&mut h, &record, 1, 5, true);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    modify(&mut h, "Fillet");
    added(&mut h);
    settled(&mut h);
    // The timeline's Block clicked: the selection is the feature alone.
    h.get_by_role_and_label(Role::Pane, "Timeline")
        .get_by_label("Block")
        .click();
    let block = silico_core::document::FeatureId::new("k7f2qx3m").unwrap();
    step_until(&mut h, "the body selected", |h| {
        h.state().tabs()[0].selection().feature() == Some(&block)
    });
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
    // One click on Edge5, which the box lists and the selection doesn't hold: removed from
    // the box, and not left in the selection.
    click_edge(&mut h, &record, 1, 5, false);
    boxed_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    for _ in 0..6 {
        step(&mut h);
    }
    assert_eq!(boxed(&h), ["k7f2qx3m.box.Edge3"]);
    assert!(selected_names(&h).is_empty(), "{:?}", selected_names(&h));
    assert_eq!(h.state().tabs()[0].selection().feature(), Some(&block));
    settled(&mut h);
    assert_eq!(
        written(&f, &h, "Fillet", "edges"),
        Some(plain(r#"["k7f2qx3m.box.Edge3"]"#))
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- The chamfer dialog (M5.8) ----

#[test]
fn a_chamfer_dialog_fed_the_selection_of_an_edge_writes_a_chamfer_of_its_distance() {
    // The document's chamfer has one size, a distance alike along both faces
    // (`ChamferSize`): its first, 1 mm (Silico's own choice), then a typed one (a 2 before
    // the 1).
    let f = fixture();
    meshes_a_real_box(&f);
    let (mut h, record) = f.drawn("box.silico", A_BOX);
    meshes_settled(&mut h);
    click_edge(&mut h, &record, 1, 3, false);
    selected_until(&mut h, &["k7f2qx3m.box.Edge3"]);
    modify(&mut h, "Chamfer");
    added(&mut h);
    settled(&mut h);
    assert!(dialog(&h).query_by_label("1 edge").is_some());
    assert!(dialog(&h).query_by_label("k7f2qx3m.box.Edge3").is_some());
    assert_eq!(input(&h, "Distance").value().as_deref(), Some("1"));
    assert_eq!(
        fields_of(&text_of(&f, &h), "Chamfer"),
        Some(expected(
            r#"{"type": "chamfer", "name": "Chamfer", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 1}}"#
        ))
    );
    type_into(&mut h, "Distance", "2");
    settled(&mut h);
    let id = dialog_feature(&h);
    dialog(&h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(&mut h, "the dialog closed", |h| !dialog_open(h));
    settled(&mut h);
    let text = text_of(&f, &h);
    assert_eq!(
        fields_of(&text, "Chamfer"),
        Some(expected(
            r#"{"type": "chamfer", "name": "Chamfer", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 12}}"#
        ))
    );
    assert!(text.contains(&format!(r#""id": "{id}""#)));
    assert_eq!(
        h.state().tabs()[0].sent,
        [Sent::Begin, Sent::Add, Sent::Edit("size".to_owned())]
    );
    assert_eq!(never_waits::waits(), 0);
}

// ---- What's Wrong ----

/// Three blocks side by side: the first and the third short enough for the kernel to refuse
/// ([`refuses_the_short_blocks`]), the second built, warned only of its missing fingerprint
/// (the document stores none: `fingerprint_missing`). Their IDs are out of their order (c,
/// a, b), so a list in the IDs' order isn't the document's.
const THREE_BLOCKS: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "c0000001", "type": "box", "v": 1, "name": "First", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10},
    {"id": "a0000001", "type": "box", "v": 1, "name": "Second", "position": [20, 0, 0], "length": 20, "width": 10, "height": 10},
    {"id": "b0000001", "type": "box", "v": 1, "name": "Third", "position": [50, 0, 0], "length": 10, "width": 10, "height": 10}
  ]
}
"#;

/// The kernel refuses every block under 15 mm long: THREE_BLOCKS' first and third.
fn refuses_the_short_blocks(f: &Fixture) {
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx < 15.0)
            .then(|| Error::new(ErrorCode::KernelFailed, "the block came apart"))
    });
}

/// THREE_BLOCKS open, rebuilt with its two errors.
fn three_blocks(f: &Fixture) -> AppHarness {
    refuses_the_short_blocks(f);
    let (mut h, _) = f.opened("blocks.silico", THREE_BLOCKS);
    step_until(&mut h, "rebuilt with its two errors", |h| {
        status_said(h, "Rebuilt").as_deref() == Some("Rebuilt with 2 errors")
    });
    h
}

/// What's Wrong, if it's open.
fn whats_wrong(h: &AppHarness) -> Option<egui_kittest::Node<'_>> {
    h.query_by_role_and_label(Role::Dialog, "What's wrong")
}

/// The labels in What's Wrong that hold `part`, in its order: none if it's closed.
fn in_whats_wrong(h: &AppHarness, part: &str) -> Vec<String> {
    whats_wrong(h)
        .map(|open| {
            open.query_all_by_label_contains(part)
                .filter_map(|node| {
                    let node = node.accesskit_node();
                    node.value().or_else(|| node.label())
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn whats_wrong_lists_two_failures_and_a_warning_in_the_documents_order_each_its_card() {
    let f = fixture();
    let mut h = three_blocks(&f);
    assert!(whats_wrong(&h).is_none());
    h.get_by_role_and_label(Role::Button, "Rebuilt with 2 errors")
        .click();
    step_until(&mut h, "three listed", |h| {
        in_whats_wrong(h, "Feature: ").len() == 3
    });
    step(&mut h);
    // Its head names the document; each card names its feature, by name and ID, in the
    // document's order (not its IDs'): First's error, Second's warning, Third's error.
    assert!(
        whats_wrong(&h)
            .unwrap()
            .query_by_label("What's wrong in blocks.silico")
            .is_some()
    );
    assert_eq!(
        in_whats_wrong(&h, "Feature: "),
        [
            "Feature: First c0000001",
            "Feature: Second a0000001",
            "Feature: Third b0000001"
        ]
    );
    // Each card's code and message, the fake's refusal and the core's warning as written by
    // hand, under that card's head: below the card before's Feature row, above its own.
    let open = whats_wrong(&h).unwrap();
    let top = |label: &str| -> Vec<f32> {
        open.query_all_by_label(label)
            .map(|node| node.rect().top())
            .collect()
    };
    let rows: Vec<f32> = open
        .query_all_by_label_contains("Feature: ")
        .map(|node| node.rect().top())
        .collect();
    assert!(rows[0] < rows[1] && rows[1] < rows[2], "{rows:?}");
    let apart = "the block came apart";
    let unprinted = "a0000001 built, but its file stores no fingerprint, so a change in what it \
                     builds isn't caught as drift";
    for (label, count) in [
        ("kernel_failed", 2),
        ("fingerprint_missing", 1),
        (apart, 2),
        (unprinted, 1),
    ] {
        assert_eq!(top(label).len(), count, "{label:?}");
    }
    for (card, (code, message)) in [
        ("kernel_failed", apart),
        ("fingerprint_missing", unprinted),
        ("kernel_failed", apart),
    ]
    .into_iter()
    .enumerate()
    {
        let above = card.checked_sub(1).map_or(f32::NEG_INFINITY, |at| rows[at]);
        for label in [code, message] {
            assert_eq!(
                top(label)
                    .into_iter()
                    .filter(|at| above < *at && *at < rows[card])
                    .count(),
                1,
                "card {card}'s {label:?}: {rows:?}"
            );
        }
    }
    // Every card offers Edit, Show in timeline, and Copy as JSON (`errCard(p)`).
    for button in ["Edit First", "Edit Second", "Edit Third"] {
        assert!(
            open.query_by_role_and_label(Role::Button, button).is_some(),
            "{button}"
        );
    }
    for button in ["Show in timeline", "Copy as JSON"] {
        assert_eq!(
            open.query_all_by_role_and_label(Role::Button, button)
                .count(),
            3,
            "{button}"
        );
    }
    // Show in timeline closes it and selects Third.
    open.query_all_by_role_and_label(Role::Button, "Show in timeline")
        .nth(2)
        .unwrap()
        .click();
    step_until(&mut h, "Third selected", |h| {
        whats_wrong(h).is_none()
            && timeline(h).get_by_label("Third").accesskit_node().toggled()
                == Some(eframe::egui::accesskit::Toggled::True)
    });
    // Edit closes it and opens First's dialog, as Edit Feature does.
    timeline(&h)
        .get_by_role_and_label(Role::Button, "2 errors")
        .click();
    step_until(&mut h, "listed again", |h| {
        in_whats_wrong(h, "Feature: ").len() == 3
    });
    step(&mut h);
    whats_wrong(&h)
        .unwrap()
        .get_by_role_and_label(Role::Button, "Edit First")
        .click();
    step_until(&mut h, "First's dialog", |h| {
        whats_wrong(h).is_none() && dialog_open(h) && dialog(h).query_by_label("BOX").is_some()
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn whats_wrong_opens_from_the_status_bar_the_timeline_and_the_documents_row_and_esc_or_close_shut_it()
 {
    let f = fixture();
    let mut h = three_blocks(&f);
    let shut = |h: &mut AppHarness, by: &str| {
        step_until(h, &format!("shut by {by}"), |h| whats_wrong(h).is_none());
        assert_eq!(h.state().tabs().len(), 1, "{by} shut only What's Wrong");
    };
    // The status bar's "Rebuilt with 2 errors"; Esc.
    h.get_by_role_and_label(Role::Button, "Rebuilt with 2 errors")
        .click();
    step_until(&mut h, "opened from the status bar", |h| {
        whats_wrong(h).is_some()
    });
    h.key_press(eframe::egui::Key::Escape);
    shut(&mut h, "Esc");
    // The timeline's "2 errors"; its Close.
    timeline(&h)
        .get_by_role_and_label(Role::Button, "2 errors")
        .click();
    // Listed, so it's done growing: a modal is centred on its size.
    step_until(&mut h, "opened from the timeline, its three listed", |h| {
        in_whats_wrong(h, "Feature: ").len() == 3
    });
    step(&mut h);
    whats_wrong(&h)
        .unwrap()
        .get_by_role_and_label(Role::Button, "Close")
        .click();
    shut(&mut h, "Close");
    // A click on the browser's document row; Esc.
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("blocks")
        .click();
    step_until(&mut h, "opened from the document's row", |h| {
        whats_wrong(h).is_some()
    });
    h.key_press(eframe::egui::Key::Escape);
    shut(&mut h, "Esc");
    // The registry's What's wrong, which they open: its tooltip's title.
    assert_eq!(
        silico_app::shortcuts::Command::WhatsWrong.label(),
        "What's wrong"
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn whats_wrong_lists_the_first_hundred_warnings_and_counts_the_thirty_past_them() {
    // 130 blocks, none with a fingerprint: 130 `fingerprint_missing` warnings, of which the
    // core lists the first 100.
    let f = fixture();
    let blocks = (0..130)
        .map(|at| {
            format!(
                r#"    {{"id": "f{at:07}", "type": "box", "v": 1, "name": "Block {at}", "position": [{x}, 0, 0], "length": 10, "width": 10, "height": 10}}"#,
                x = at * 20
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let text = format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n{blocks}\n  ]\n}}\n"
    );
    let (mut h, _) = f.opened("many.silico", &text);
    step_until(&mut h, "the timeline warned", |h| {
        h.state().tabs()[0]
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.features.as_ref())
            .is_some_and(|features| features.iter().all(|entry| entry.warnings == 1))
    });
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("many")
        .click();
    step_until(&mut h, "a hundred listed", |h| {
        in_whats_wrong(h, "Feature: ").len() == 100
    });
    step(&mut h);
    let open = whats_wrong(&h).unwrap();
    assert_eq!(open.query_all_by_label("fingerprint_missing").count(), 100);
    // The first hundred, in the document's order: Block 0 to Block 99.
    let features = in_whats_wrong(&h, "Feature: ");
    assert_eq!(features[0], "Feature: Block 0 f0000000");
    assert_eq!(features[99], "Feature: Block 99 f0000099");
    // Then the thirty past them, counted; no errors to count.
    assert!(open.query_by_label("and 30 more warnings").is_some());
    assert_eq!(open.query_all_by_label_contains(" more error").count(), 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn on_lavapipe_a_features_card_is_titled_by_its_name_and_its_message_wraps_across_the_body() {
    on_lavapipe(
        "on_lavapipe_a_features_card_is_titled_by_its_name_and_its_message_wraps_across_the_body",
        || {
            let f = fixture();
            meshes_a_real_box(&f);
            refuses_the_short_blocks(&f);
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
            h.set_render_every_step(true);
            silico_app::theme::apply(&h.ctx);
            let path = f.file("blocks.silico", THREE_BLOCKS);
            h.state_mut().open(path.to_str().unwrap(), false);
            step_until(&mut h, "rebuilt with its two errors", |h| {
                status_said(h, "Rebuilt").as_deref() == Some("Rebuilt with 2 errors")
            });
            meshes_settled(&mut h);
            select(&mut h, "Second");
            for _ in 0..3 {
                step(&mut h);
            }
            // The head: the feature's name beside the code; the core's message under them,
            // in the body, starting at the body's left.
            let pane = h.get_by_role_and_label(Role::Pane, "Properties");
            let code = pane.get_by_label("fingerprint_missing").rect();
            let title = pane
                .query_all_by_label("Second")
                .map(|node| node.rect())
                .find(|rect| (rect.top() - code.top()).abs() <= 0.5)
                .expect("the title on the code's line");
            let message = pane
                .query_all_by_label_contains("built, but its file stores no fingerprint")
                .next()
                .expect("the message")
                .rect();
            assert!(message.top() >= code.bottom(), "{message:?} under {code:?}");
            assert!(message.left() < code.left() && title.right() < code.left());
            if let Some(shot) = std::env::var_os("SILICO_APP_CARD_SHOT") {
                h.render().unwrap().save(shot).unwrap();
            }
            assert_eq!(never_waits::waits(), 0);
        },
    );
}

#[test]
fn on_lavapipe_whats_wrong_shows_two_error_cards_and_a_warnings() {
    on_lavapipe(
        "on_lavapipe_whats_wrong_shows_two_error_cards_and_a_warnings",
        || {
            let f = fixture();
            meshes_a_real_box(&f);
            refuses_the_short_blocks(&f);
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
            h.set_render_every_step(true);
            silico_app::theme::apply(&h.ctx);
            let path = f.file("blocks.silico", THREE_BLOCKS);
            h.state_mut().open(path.to_str().unwrap(), false);
            step_until(&mut h, "rebuilt with its two errors", |h| {
                status_said(h, "Rebuilt").as_deref() == Some("Rebuilt with 2 errors")
            });
            meshes_settled(&mut h);
            h.get_by_role_and_label(Role::Button, "Rebuilt with 2 errors")
                .click();
            step_until(&mut h, "three listed", |h| {
                in_whats_wrong(h, "Feature: ").len() == 3
            });
            for _ in 0..3 {
                step(&mut h);
            }
            assert_eq!(
                whats_wrong(&h)
                    .unwrap()
                    .query_all_by_label("kernel_failed")
                    .count(),
                2
            );
            if let Some(shot) = std::env::var_os("SILICO_APP_WHATS_WRONG_SHOT") {
                h.render().unwrap().save(shot).unwrap();
            }
            assert_eq!(never_waits::waits(), 0);
        },
    );
}
