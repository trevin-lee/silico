//! Hostile input for the Undo and Redo tooltips' step names and the status bar's count of
//! unsaved edits (the prototype's `data-tip`, `'Undo: ' + label`, and `renderStatus`'s
//! `Unsaved · N edits`): a feature named past the screen's width, a name across two lines,
//! Undo and Redo pressed faster than the session answers them, an undo waiting behind a held
//! rebuild, a file changed on disk under held edits (kept, then reloaded), a revert, and a
//! dialog's edits redone after its Cancel. Driven as the app's own tests drive it, through
//! `egui_kittest` on the fake kernel, every frame on a thread marked so that a wait on a
//! command fails. Expected values by hand: each name the prototype's grammar gives it, each
//! count the steps worked out at each point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/ui.rs"]
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use silico_app::App;
use silico_app::tab::Sent;
use silico_core::command::{CheckModel, Sessions};
use silico_core::disk::Clock;
use silico_core::fake::{FakeKernel, FakeSketches, never_waits};
use tempfile::TempDir;
use ui::{
    EXAMPLE, SKETCHED_EXAMPLE, apply_look, in_status_bar, plain_read, redo_by_key, settled,
    stand_in_sessions, step, step_until, tooltip, tooltip_title, type_name, undo_by_key,
};

type Fake = Arc<FakeKernel>;
type AppHarness = Harness<'static, App<Fake>>;

struct Fixture {
    dir: TempDir,
    kernel: Fake,
    sessions: Arc<Sessions<Fake>>,
}

/// Sessions on the fake kernel, each file compared on disk by its bytes, at once.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let now: Clock = Arc::new(Instant::now);
    let sessions = stand_in_sessions(&dir, &kernel, &FakeSketches::new())
        .watching_disk(Duration::ZERO, Arc::new(plain_read), now)
        .unwrap();
    Fixture {
        dir,
        kernel,
        sessions: Arc::new(sessions),
    }
}

impl Fixture {
    /// The app with `text` open from `name`, its timeline read.
    fn opened(&self, name: &str, text: &str) -> (AppHarness, PathBuf) {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let app = App::new(Arc::clone(&self.sessions));
        let _marked = never_waits::marked();
        let mut h = Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app);
        apply_look(&h);
        h.state_mut().open(path.to_str().unwrap(), false);
        step_until(&mut h, "the timeline read", |h| {
            h.state()
                .tabs()
                .first()
                .is_some_and(|tab| tab.timeline.is_some())
        });
        (h, path)
    }

    /// The session's document text.
    fn text(&self, h: &AppHarness) -> String {
        let id = h.state().tabs()[0].id;
        self.sessions.text(&id, None).wait().result.unwrap().text
    }
}

/// The feature `name` selected on the timeline, and `text` typed after its name in the
/// property panel's Name field, one keystroke a frame: one rename.
fn renamed(h: &mut AppHarness, name: &str, text: &str) {
    ui::select(h, name);
    type_name(h, text);
    settled(h);
}

/// The status bar's words of whether the document is saved: "Saved", "Unsaved", or
/// "Unsaved · N edit(s)".
fn saved_said(h: &AppHarness) -> Option<String> {
    let bar = h.get_by_role_and_label(Role::Button, "Shortcuts").rect();
    h.query_all_by_role(Role::Label).find_map(|node| {
        let rect = node.rect();
        let node = node.accesskit_node();
        let said = node.label().or_else(|| node.value())?;
        (bar.y_range().contains(rect.center().y)
            && (said == "Saved" || said.starts_with("Unsaved")))
        .then_some(said)
    })
}

#[test]
fn a_feature_named_past_the_screens_width_is_cut_short_in_the_tooltip_and_kept_on_screen() {
    // A pin named 400 W's (W, the widest letter): its rename named by the name it replaced,
    // cut at the app's 60 characters with an ellipsis (`changes::SHOWN_CHARS`, as the
    // reload banner's rows cut a name), so the tooltip stays inside the 1280-point window.
    let f = fixture();
    let long = "W".repeat(400);
    let text = EXAMPLE.replace(r#""name": "Pin""#, &format!(r#""name": "{long}""#));
    let (mut h, _) = f.opened("long.silico", &text);
    step_until(&mut h, "its features", |h| {
        h.get_by_role_and_label(Role::Pane, "Timeline")
            .query_by_label(&long)
            .is_some()
    });
    renamed(&mut h, &long, "x");
    let (title, rect) = tooltip(&mut h, "Undo");
    assert_eq!(title, format!("Undo: Rename {}…", "W".repeat(60)));
    assert!(rect.right() <= 1280.0 && rect.left() >= 0.0, "{rect:?}");
    // And Redo once it's undone, the same.
    undo_by_key(&mut h);
    let (title, rect) = tooltip(&mut h, "Redo");
    assert_eq!(title, format!("Redo: Rename {}…", "W".repeat(60)));
    assert!(rect.right() <= 1280.0 && rect.left() >= 0.0, "{rect:?}");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_name_across_two_lines_is_named_on_one_as_the_prototypes_tooltip_shows_it() {
    // A pin named "Pin\nTop" in the file (a hand edit: the app's own fields take one
    // line), suppressed. The prototype's tooltip is HTML, whose text collapses the line
    // break to a space: "Undo: Suppress Pin Top", one line.
    let f = fixture();
    let text = EXAMPLE.replace(r#""name": "Pin""#, r#""name": "Pin\nTop""#);
    let (mut h, _) = f.opened("lines.silico", &text);
    step_until(&mut h, "its features", |h| {
        h.state().tabs()[0].timeline.is_some()
    });
    // Suppressed, as its menu's Suppress asks it.
    {
        let sessions = Arc::clone(&f.sessions);
        let tab = &mut h.state_mut().tabs_mut()[0];
        tab.suppress_feature(
            &sessions,
            silico_core::document::FeatureId::new("a91cw0de").unwrap(),
            true,
        );
    }
    settled(&mut h);
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Suppress Pin Top");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn undo_and_redo_pressed_one_a_frame_leave_the_names_and_the_count_of_the_state_reached() {
    // Three renames (Pin, Hole, Block, in that order), then Ctrl+Z, Ctrl+Z, Ctrl+Y, Ctrl+Z,
    // Ctrl+Z, Ctrl+Y, Ctrl+Y, Ctrl+Z, one a frame, none waited for: the tab takes those
    // that come while no step of its is under way (one at least, the first), and drops the
    // rest. Once everything has answered, the names and the count are those of the state
    // the document is at: k renames standing, Undo names the k-th, Redo the next.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| {
        h.state().tabs()[0].timeline.is_some()
    });
    renamed(&mut h, "Pin", "p");
    renamed(&mut h, "Hole", "h");
    renamed(&mut h, "Block", "b");
    for undo in [true, true, false, true, true, false, false, true] {
        let key = if undo { Key::Z } else { Key::Y };
        h.key_press_modifiers(Modifiers::COMMAND, key);
        step(&mut h);
    }
    settled(&mut h);
    let (undone, redone) =
        h.state().tabs()[0]
            .sent
            .iter()
            .fold((0, 0), |(u, r), sent| match sent {
                Sent::Undo(_) => (u + 1, r),
                Sent::Redo(_) => (u, r + 1),
                _ => (u, r),
            });
    assert!(
        (1..=8).contains(&(undone + redone)),
        "{undone} undos and {redone} redos taken of 8 pressed"
    );
    step_until(&mut h, "the status of the state reached", |h| {
        let tab = &h.state().tabs()[0];
        !tab.unsettled() && tab.status.is_some()
    });
    for _ in 0..4 {
        step(&mut h);
    }
    let text = f.text(&h);
    let standing = ["Pinp", "Holeh", "Blockb"]
        .iter()
        .take_while(|name| text.contains(&format!(r#""name": "{name}""#)))
        .count();
    // The steps taken are what moved it: from three, one less for each undo, one more for
    // each redo.
    assert_eq!(
        standing,
        3 + redone - undone,
        "{undone} undone, {redone} redone"
    );
    // Renames undo in order: whatever stands is a prefix of the three.
    for (at, name) in ["Pinp", "Holeh", "Blockb"].iter().enumerate() {
        assert_eq!(
            text.contains(&format!(r#""name": "{name}""#)),
            at < standing,
            "{name} with {standing} standing"
        );
    }
    let renames = ["Rename Pin", "Rename Hole", "Rename Block"];
    let undo = match standing {
        0 => "Undo".to_owned(),
        k => format!("Undo: {}", renames[k - 1]),
    };
    let redo = match renames.get(standing) {
        Some(step) => format!("Redo: {step}"),
        None => "Redo".to_owned(),
    };
    assert_eq!(tooltip_title(&mut h, "Undo"), undo, "{standing} standing");
    assert_eq!(tooltip_title(&mut h, "Redo"), redo, "{standing} standing");
    let said = match standing {
        0 => "Saved".to_owned(),
        1 => "Unsaved · 1 edit".to_owned(),
        k => format!("Unsaved · {k} edits"),
    };
    assert_eq!(saved_said(&h).as_deref(), Some(said.as_str()));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_undo_waiting_behind_a_held_rebuild_names_nothing_till_it_answers_then_the_step_before() {
    // Two renames; then Rebuild everything held in the kernel, and Ctrl+Z, which waits
    // behind it in the session's queue: while it waits, Undo and Redo name nothing (its
    // answer moves what each would take). Released: Undo names the pin's rename, Redo the
    // hole's, and one edit is counted.
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| {
        h.state().tabs()[0].timeline.is_some()
    });
    renamed(&mut h, "Pin", "p");
    renamed(&mut h, "Hole", "h");
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Rename Hole");
    let hold = f.kernel.hold(|_| true);
    let id = h.state().tabs()[0].id;
    let check = f.sessions.check(&id, CheckModel::Current { fresh: true });
    assert!(f.kernel.wait_running(1, Duration::from_secs(60)));
    step_until(&mut h, "undoable", |h| h.state().tabs()[0].undoable());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    step(&mut h);
    assert!(h.state().tabs()[0].unsettled(), "the undo didn't wait");
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo");
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo");
    hold.release();
    let _ = check.wait();
    step_until(&mut h, "the undo answered", |h| {
        !h.state().tabs()[0].unsettled() && h.state().tabs()[0].redoable()
    });
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Rename Pin");
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo: Rename Hole");
    step_until(&mut h, "one counted", |h| {
        in_status_bar(h, "Unsaved · 1 edit")
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_file_changed_under_held_edits_keeps_their_names_if_kept_and_drops_them_if_reloaded() {
    let f = fixture();
    let (mut h, path) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| {
        h.state().tabs()[0].timeline.is_some()
    });
    renamed(&mut h, "Pin", "p");
    renamed(&mut h, "Hole", "h");
    std::fs::write(&path, EXAMPLE.replace("Clearance", "Room")).unwrap();
    let said = "example.silico changed on disk while you have unsaved edits.";
    step_until(&mut h, "the change announced", |h| ui::anywhere(h, said));
    // Kept: nothing of the stack changes.
    h.get_by_role_and_label(Role::Button, "Keep my edits…")
        .click();
    step_until(&mut h, "the banner gone", |h| !ui::anywhere(h, said));
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Rename Hole");
    assert_eq!(saved_said(&h).as_deref(), Some("Unsaved · 2 edits"));
    // Reloaded: the edits rolled back and the file read, a new document under no stack:
    // nothing to name or count.
    h.get_by_role_and_label(Role::Button, "Changed on disk")
        .click();
    step_until(&mut h, "the choice offered again", |h| {
        ui::anywhere(h, said)
    });
    h.get_by_role_and_label(Role::Button, "Reload, discard my edits")
        .click();
    step_until(&mut h, "reloaded", |h| {
        f.sessions
            .text(&h.state().tabs()[0].id, None)
            .wait()
            .result
            .is_ok_and(|text| text.text.contains("Room"))
            && !h.state().tabs()[0].transaction
    });
    settled(&mut h);
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo");
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo");
    step_until(&mut h, "saved", |h| {
        saved_said(h).as_deref() == Some("Saved")
    });
    // The next edit is the first of a new stack.
    renamed(&mut h, "Pin", "q");
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Rename Pin");
    step_until(&mut h, "one counted", |h| {
        saved_said(h).as_deref() == Some("Unsaved · 1 edit")
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn revert_drops_every_step_and_the_next_edit_counts_alone() {
    let f = fixture();
    let (mut h, _) = f.opened("example.silico", EXAMPLE);
    step_until(&mut h, "its features", |h| {
        h.state().tabs()[0].timeline.is_some()
    });
    renamed(&mut h, "Pin", "p");
    renamed(&mut h, "Hole", "h");
    undo_by_key(&mut h);
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo: Rename Hole");
    h.get_by_label("File").click();
    step(&mut h);
    step(&mut h);
    h.get_by_role_and_label(Role::Button, "Revert").click();
    step_until(&mut h, "reverted", |h| !h.state().tabs()[0].transaction);
    settled(&mut h);
    assert_eq!(f.text(&h), EXAMPLE);
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo");
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo");
    renamed(&mut h, "Block", "b");
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Rename Block");
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo");
    step_until(&mut h, "one counted", |h| {
        saved_said(h).as_deref() == Some("Unsaved · 1 edit")
    });
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_cancelled_dialogs_edits_redone_one_by_one_are_named_by_the_dialog() {
    // The pin renamed first, so the pad's dialog opens inside a transaction and its Cancel
    // undoes back to where it opened, leaving its add and its length on the redo branch.
    // Redo gives each back alone (no OK made them one step), each named as the dialog
    // names its edits: its add "Add Pad", its length "Edit Pad".
    let f = fixture();
    let (mut h, _) = f.opened("sketched.silico", SKETCHED_EXAMPLE);
    step_until(&mut h, "its features", |h| {
        h.state().tabs()[0].timeline.is_some()
    });
    renamed(&mut h, "Pin", "K");
    step_until(&mut h, "its sketch", |h| {
        h.get_by_role_and_label(Role::Pane, "Browser")
            .query_by_label("Triangle")
            .is_some()
    });
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label("Triangle")
        .click();
    step(&mut h);
    ui::create(&mut h, "Pad");
    ui::added(&mut h);
    ui::type_over(&mut h, ui::In::Dialog, "Length", "12");
    settled(&mut h);
    ui::dialog(&h)
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    step_until(&mut h, "the dialog closed", |h| !ui::dialog_open(h));
    settled(&mut h);
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Rename Pin");
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo: Add Pad");
    redo_by_key(&mut h);
    assert_eq!(tooltip_title(&mut h, "Undo"), "Undo: Add Pad");
    assert_eq!(tooltip_title(&mut h, "Redo"), "Redo: Edit Pad");
    assert_eq!(saved_said(&h).as_deref(), Some("Unsaved · 2 edits"));
    assert_eq!(never_waits::waits(), 0);
}
