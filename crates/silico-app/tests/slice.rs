//! M5.12's vertical slice (docs/roadmap/m5-app.md, *M5.12*): a scripted session builds M4.9's
//! bracket (crates/silico-cli/tests/bracket/spec.md) in the app from its spec, through the
//! app's UI alone, on the real kernel, sketch, and STEP workers and the real renderer on
//! Mesa's lavapipe: sketch, pad, pocket the holes, fillet the inside corner; then saves,
//! rebuilds everything, and exports.
//!
//! **Driven as a person drives it.** Every step is a click, a key, or typed text: through
//! the accessibility tree, or the canvas's own pointer events (a sketch's corners clicked at
//! the screen places the frame's own camera shows them at, the fillet's edge clicked where
//! the renderer draws it and found in lavapipe's ID buffer). Only reads are taken from the
//! app's state (where the frame shows a place, what a read holds, what the tab answered),
//! never a method that acts. Where the views put the sketch under the app's own overlays,
//! the session pans and zooms the canvas as a person would ([`clear_view`]).
//!
//! **The calls the done test leaves open, and how they're made here:**
//! - *The MCP bracket* is the recorded session's (crates/silico-cli/tests/bracket/
//!   session.jsonl, its features as silico-core's tests/common/bracket.rs holds them):
//!   sketch, pad, sketch, pocket, sketch, pocket, fillet. Not the live agent's run
//!   (docs/walkthrough/m4-live/bracket.silico), which builds the same part another way
//!   (boxes fused and filleted, then pocketed).
//! - *Its fingerprints, feature for feature.* A fingerprint holds no ID, no name and no
//!   sketch member, and a sketch has none: so the UI's pad, pockets and fillet, in timeline
//!   order, each against the recorded session's, which the test rebuilds on the same kernel
//!   (the session's features written to a file and formatted by a core session of its own,
//!   as bracket.rs's timeline test does), field for field; and both against the hand. The
//!   sketches are compared by what they store: their points and their circles' radii.
//! - *Within the tolerance's bound*: bracket.rs's exact one, the chords counted by hand from
//!   OCCT's rule and the mesh's volume the part's and its chords' segments', within the
//!   coordinates' 32-bit rounding (silico-cli's tests/common/hand.rs).
//! - *Names.* Each feature is renamed with F2 to the session's name as it's made, so the
//!   3MF's object and the STEP's product read "Inside corner".
//! - *Constraints.* The tools write their own relations (the UI's L is level and plumb with
//!   the origin, `on` the axes, where the session wrote distances of 0; two diameters where it
//!   wrote a diameter and `equal`, which isn't built), so the constraint sets differ; the
//!   geometry they solve to is compared, exactly.
//!
//! **Checked by hand,** never read from the app: each feature's volume, area, centre, box
//! and counts (the hand's sums), the fillet's edge by its name built from the document's own
//! IDs, the 3MF as bracket.rs checks it (printable, 1,084 vertices and 2,180 triangles, its
//! box, its volume), the STEP as FreeCAD 1.1.3 (the oracle) reads it back with `Part.read`
//! and `Import.insert`, as silico-oracle's STEP tests read theirs, and `.stp` written byte
//! for byte as `.step`.
//!
//! **Needs** the workers built beside the test (`cargo build --workspace --bins`, or
//! `pixi run test`: `-p silico-app` alone builds none of them), lavapipe (pixi's), and the
//! FreeCAD oracle (`pixi install -e oracle`). Its name holds "oracle" for nextest's longer
//! limit (.config/nextest.toml).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// The bracket's features as the MCP session added them.
#[path = "../../silico-core/tests/common/bracket.rs"]
mod features;
#[path = "../../silico-cli/tests/common/hand.rs"]
mod hand;
#[path = "../../silico-cli/tests/common/threemf.rs"]
mod threemf;
#[path = "common/ui.rs"]
mod ui;

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::accesskit::Role;
use eframe::egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::Value;
use silico_app::App;
use silico_app::sketch::screen::Screen;
use silico_core::cache::Cache;
use silico_core::command::Sessions;
use silico_core::fake::never_waits;
use silico_core::pool::KernelPool;
use silico_core::sketch_states::{ConstraintState, SketchState};
use silico_render::render::project;

use hand::{
    FILLET_R, HOLE, HOLE_R, PI, Sum, bracket, centred, chords, drilled, enclosed, legs, measured,
    on_circle, rounding,
};
use threemf::{model, printable};
use ui::*;

type H = AppUi<KernelPool>;

/// The export's tolerance: the dialog's default, FreeCAD's, the session's.
const TOLERANCE: f64 = 0.1;

thread_local! {
    /// How many Modify boxes didn't take the keyboard as they opened (finding N4).
    static UNFOCUSED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

const NAME: &str =
    "the_bracket_built_in_the_app_exports_a_printable_3mf_and_a_step_the_oracle_reads_back";

// --- The canvas, through the frame's own camera ---------------------------------------------

/// Where the open sketch's place `at` (its millimetres) is on the screen, as the last frame's
/// camera shows it: a read of the frame, the projection the renderer draws with.
fn seen(h: &H, at: [f64; 2]) -> Pos2 {
    let read = sketch_mode(h).unwrap().drawable().unwrap();
    let screen = Screen::of(read, frame(h).unwrap()).unwrap();
    screen.project(at).unwrap()
}

/// The rectangles of what the app draws over the canvas: the ViewCube, the navigation bar,
/// and in sketch mode the sketch bar and the Sketch Palette, each the bounds of what it
/// holds (the groups themselves have none). A click there is theirs.
fn overlays(h: &H) -> Vec<(&'static str, Rect)> {
    let mut found = Vec::new();
    for label in ["Sketch bar", "Sketch Palette", "ViewCube", "Navigation bar"] {
        for node in h.query_all_by_label(label) {
            let mut bounds = Rect::NOTHING;
            for inner in node.query_all(egui_kittest::kittest::by().recursive(true)) {
                if let Some(b) = inner.accesskit_node().bounding_box() {
                    let rect = Rect::from_min_max(
                        Pos2::new(b.x0 as f32, b.y0 as f32),
                        Pos2::new(b.x1 as f32, b.y1 as f32),
                    );
                    if rect.is_finite() {
                        bounds = bounds.union(rect);
                    }
                }
            }
            if bounds.is_positive() {
                found.push((label, bounds));
            }
        }
    }
    found
}

/// The canvas less what the overlays take from its sides: each overlay taken off the side
/// it stands on, the one that leaves the most room.
fn free(h: &H) -> Rect {
    let mut room = frame(h).unwrap().rect;
    for (_, o) in overlays(h) {
        let cuts = [
            Rect::from_min_max(room.min, Pos2::new(room.max.x, room.max.y.min(o.min.y))),
            Rect::from_min_max(Pos2::new(room.min.x, room.min.y.max(o.max.y)), room.max),
            Rect::from_min_max(room.min, Pos2::new(room.max.x.min(o.min.x), room.max.y)),
            Rect::from_min_max(Pos2::new(room.min.x.max(o.max.x), room.min.y), room.max),
        ];
        if let Some(best) = cuts
            .into_iter()
            .filter(|cut| !cut.intersects(o.shrink(0.5)))
            .max_by(|a, b| a.area().total_cmp(&b.area()))
        {
            room = best;
        }
    }
    room
}

/// Whether the screen place `at` is on the canvas, a margin of `room` points clear of its
/// edges and of every overlay.
fn clear(h: &H, at: Pos2, room: f32) -> bool {
    let canvas = frame(h).unwrap().rect.shrink(room);
    canvas.contains(at)
        && overlays(h)
            .iter()
            .all(|(_, rect)| !rect.expand(room).contains(at))
}

/// The view moved as a person would until every sketch place in `places` is on the canvas
/// and clear of the overlays: the canvas dragged with the middle button, from the middle of
/// the room the overlays leave, so the places' middle comes there; then the wheel, a notch at
/// a time there, zooming out while any is under something. How many notches it took.
fn clear_view(h: &mut H, places: &[[f64; 2]]) -> u32 {
    let lo = places
        .iter()
        .fold([f64::INFINITY; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
    let hi = places.iter().fold([f64::NEG_INFINITY; 2], |m, p| {
        [m[0].max(p[0]), m[1].max(p[1])]
    });
    let middle = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let centre = free(h).center();
    let by = centre - seen(h, middle);
    drag(h, PointerButton::Middle, centre, centre + by, 8);
    settle_frame(h);
    let mut notches = 0;
    while !places.iter().all(|p| clear(h, seen(h, *p), 12.0)) {
        assert!(
            notches < 20,
            "never clear: {:?} on {:?}",
            places.iter().map(|p| seen(h, *p)).collect::<Vec<_>>(),
            overlays(h)
        );
        wheel(h, centre, MouseWheelUnit::Line, -1.0);
        settle_frame(h);
        notches += 1;
    }
    notches
}

/// Frames stepped until the viewport's camera has stayed the same over two of them.
fn settle_frame(h: &mut H) {
    let camera = |h: &H| frame(h).map(|f| (f.camera.clone(), f.rect));
    let last = std::cell::RefCell::new(camera(h));
    step_until(h, "the view still", |h| {
        let now = camera(h);
        now == last.replace(now.clone())
    });
    step(h);
}

/// A standard view chosen by its key, Command and `key` (Command+1, Front), and stepped
/// until the frame shows it: looking along `looking` (Front along +Y).
fn standard_view(h: &mut H, key: Key, looking: [f64; 3]) {
    h.key_press_modifiers(Modifiers::COMMAND, key);
    let along = |h: &H| {
        frame(h).is_some_and(|f| {
            let d: [f64; 3] = std::array::from_fn(|k| f.camera.target[k] - f.camera.eye[k]);
            let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let l = (looking[0] * looking[0] + looking[1] * looking[1] + looking[2] * looking[2])
                .sqrt();
            (0..3).all(|k| (d[k] / n - looking[k] / l).abs() < 1e-9)
        })
    };
    step_until(h, "the view chosen", along);
    settle_frame(h);
}

/// A click at the open sketch's place `at`, where the frame shows it, clear of the overlays.
fn sketch_click(h: &mut H, at: [f64; 2]) {
    let place = seen(h, at);
    assert!(
        clear(h, place, 4.0),
        "{at:?} at {place:?}, under {:?}",
        overlays(h)
    );
    click_at(h, place, false);
}

/// Stepped until sketch mode's newest read holds what `holds` asks of its members; failing,
/// saying what the read holds.
fn read_until(h: &mut H, what: &str, holds: impl Fn(&silico_core::document::Sketch) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(h);
        let read = sketch_mode(h).and_then(|mode| mode.drawable());
        if read.is_some_and(|read| holds(&read.members)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never {what}; the read holds {:?}",
            read.map(|read| (
                geometry(&read.members),
                read.members
                    .lines
                    .iter()
                    .map(|l| format!("{} {}-{}", l.id, l.from, l.to))
                    .collect::<Vec<_>>(),
                read.members
                    .constraints
                    .iter()
                    .map(|c| c.form.type_name())
                    .collect::<Vec<_>>(),
                read.states.dofs,
            ))
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The Modify box's field.
fn modify_field(h: &H) -> Option<egui_kittest::Node<'_>> {
    h.query_by_role_and_label(Role::Dialog, "Modify dimension")
        .and_then(|d| d.query_by_label("Value in millimetres"))
}

/// Smart Dimension's pick at `pick`, then its click on nothing at `place`; the Modify box,
/// once open, typed over with `value` (Home, a Delete for each character it holds, then the
/// value), and Enter; stepped until the read has `constraints` constraints, and checked
/// that no over-define prompt opened.
fn dimension(h: &mut H, pick: &[[f64; 2]], place: [f64; 2], value: &str, constraints: usize) {
    for at in pick {
        sketch_click(h, *at);
    }
    sketch_click(h, place);
    // Open; it takes the keyboard as it opens. Where it doesn't (a later sketch's: finding
    // N4), clicked into, as a person would before typing.
    step_until(h, "the Modify box open", |h| modify_field(h).is_some());
    step(h);
    step(h);
    if !modify_field(h).unwrap().is_focused() {
        let at = modify_field(h).unwrap().rect().center();
        click_at(h, at, false);
        step_until(h, "the Modify box holding the keyboard", |h| {
            modify_field(h).is_some_and(|field| field.is_focused())
        });
        UNFOCUSED.with(|n| n.set(n.get() + 1));
    }
    let held = modify_field(h)
        .unwrap()
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
    modify_field(h).unwrap().type_text(value);
    step(h);
    step(h);
    assert_eq!(
        modify_field(h).and_then(|f| f.value()).as_deref(),
        Some(value)
    );
    h.key_press(Key::Enter);
    step(h);
    step_until(h, "the read after the dimension", |h| {
        sketch_mode(h).is_some_and(|mode| {
            mode.drawable()
                .is_some_and(|read| read.members.constraints.len() == constraints)
                && (!mode.tools.busy() || mode.offer().is_some())
        })
    });
    for _ in 0..3 {
        step(h);
    }
    // G5's risk: the solve's diagnosis flags a relation the tools inferred, and the tool
    // offers to undo a dimension that over-defines nothing.
    let offer = sketch_mode(h).unwrap().offer().is_some();
    assert!(
        prompt(h).is_none() && !offer,
        "the over-define prompt opened for {value} picked at {pick:?}: {:?}",
        sketch_mode(h)
            .unwrap()
            .drawable()
            .map(|r| &r.states.constraints)
    );
}

/// The newest read's constraints that its diagnosis flags, by their form's type.
fn flagged(h: &H) -> Vec<(String, ConstraintState)> {
    let read = sketch_mode(h).unwrap().drawable().unwrap();
    read.states
        .constraints
        .iter()
        .filter(|(_, state)| **state != ConstraintState::Satisfied)
        .map(|(id, state)| {
            let form = read
                .members
                .constraints
                .iter()
                .find(|c| &c.id == id)
                .map_or("?", |c| c.form.type_name());
            (format!("{id} {form}"), *state)
        })
        .collect()
}

/// The open sketch fully defined, with no freedom left: its read's state, and the palette's.
fn fully_defined(h: &mut H) {
    step_until(h, "fully defined", |h| {
        sketch_mode(h)
            .and_then(|mode| mode.drawable())
            .is_some_and(|read| {
                (read.states.state, read.states.dofs) == (SketchState::FullyDefined, 0)
            })
    });
    step(h);
    assert!(
        h.get_by_role_and_label(Role::Group, "Sketch Palette")
            .query_all_by_label("Fully defined")
            .count()
            >= 1
    );
}

/// The newest read's points, each as (x, y), and its circles' centres and radii, sorted.
fn geometry(sketch: &silico_core::document::Sketch) -> (Vec<[f64; 2]>, Vec<([f64; 2], f64)>) {
    let at = |id: &silico_core::document::ElementId| {
        let p = sketch.points.iter().find(|p| &p.id == id).unwrap();
        [p.x.mm(), p.y.mm()]
    };
    let mut points: Vec<[f64; 2]> = sketch.points.iter().map(|p| [p.x.mm(), p.y.mm()]).collect();
    points.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut circles: Vec<([f64; 2], f64)> = sketch
        .circles
        .iter()
        .map(|c| (at(&c.center), c.radius.mm()))
        .collect();
    circles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (points, circles)
}

// --- The timeline ------------------------------------------------------------------------

/// The timeline's entries, as the tab last read them: each ID, type, name, problems and
/// warnings.
fn entries(h: &H) -> Vec<(String, &'static str, String, u64, u64)> {
    h.state().tabs()[0]
        .timeline
        .as_ref()
        .and_then(|t| t.features.as_ref())
        .map(|features| {
            features
                .iter()
                .map(|e| {
                    (
                        e.id.to_string(),
                        e.kind,
                        e.name.clone(),
                        e.problems,
                        e.warnings,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every feature on the timeline built with no problem, and the document none: settled.
fn healthy(h: &mut H, features: usize) {
    settled(h);
    step_until(h, "every feature read", |h| entries(h).len() == features);
    let timeline = h.state().tabs()[0].timeline.as_ref().unwrap();
    assert_eq!(timeline.problems, 0);
    for (id, kind, name, problems, _) in entries(h) {
        assert_eq!(problems, 0, "{name} ({kind} {id})");
    }
}

/// The feature named `from` renamed `to` with F2 on the timeline, and Enter.
fn rename(h: &mut H, from: &str, to: &str) {
    let id = entries(h)
        .into_iter()
        .find(|(_, _, name, _, _)| name == from)
        .unwrap()
        .0;
    f2_typed(h, from, to);
    h.key_press(Key::Enter);
    step_until(h, "renamed", |h| {
        h.state().tabs()[0].edits.idle()
            && entries(h)
                .iter()
                .any(|(i, _, name, _, _)| *i == id && name == to)
            && rename_field(h, to).is_none()
    });
    settled(h);
}

/// The browser's row `name` clicked.
fn in_browser(h: &mut H, name: &str) {
    h.get_by_role_and_label(Role::Pane, "Browser")
        .get_by_label(name)
        .click();
    step(h);
    step(h);
}

/// The dialog's OK clicked, and the dialog closed, everything answered.
fn ok(h: &mut H) {
    settled(h);
    assert!(ok_enabled(h));
    dialog(h).get_by_role_and_label(Role::Button, "OK").click();
    step_until(h, "the dialog closed", |h| !dialog_open(h));
    settled(h);
}

/// A sweep (Pad, Pocket) of the sketch named `sketch`: chosen in the browser, its dialog
/// opened from CREATE, its add answered.
fn sweep(h: &mut H, kind: &str, sketch: &str) {
    in_browser(h, sketch);
    create(h, kind);
    added(h);
    assert!(dialog(h).query_by_label(sketch).is_some());
}

// --- The done test's references ------------------------------------------------------------

/// The MCP session's seven features, written to a file in a folder of its own in `dir` and
/// formatted (`write`) by a core session of its own on the real kernel, as bracket.rs's
/// timeline test does: the document they save, each solid's fingerprint stored.
fn recorded(dir: &Path) -> Value {
    use features::*;
    let dir = dir.join("recorded");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("bracket.silico");
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        [
            S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001
        ]
        .join(", ")
    );
    std::fs::write(&path, text).unwrap();
    let secret = dir.join("secret");
    let sessions = Sessions::with_cache_opener(KernelPool::start(), move |cache| {
        Cache::with_secret(cache, &secret)
    });
    let opened = sessions.open(&path, false).wait().result.unwrap();
    let formatted = sessions
        .format(&opened.session, None, true)
        .wait()
        .result
        .unwrap();
    assert!(formatted.written);
    sessions.close(&opened.session).wait().result.unwrap();
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// The features of `document` of types `kinds`, in timeline order.
fn of_kinds<'a>(document: &'a Value, kinds: &[&str]) -> Vec<&'a Value> {
    document["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| kinds.contains(&f["type"].as_str().unwrap()))
        .collect()
}

/// A stored sketch's points and its circles' centres and radii, sorted.
fn stored(sketch: &Value) -> (Vec<[f64; 2]>, Vec<([f64; 2], f64)>) {
    let points = sketch["points"].as_array().unwrap();
    let at = |id: &Value| {
        let p = points.iter().find(|p| p["id"] == *id).unwrap();
        [p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap()]
    };
    let mut places: Vec<[f64; 2]> = points
        .iter()
        .map(|p| [p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap()])
        .collect();
    places.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut circles: Vec<([f64; 2], f64)> = sketch["circles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (at(&c["center"]), c["radius"].as_f64().unwrap()))
        .collect();
    circles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (places, circles)
}

/// A fingerprint against the hand: its volume, area and centre to the measure's bounds,
/// its box and its counts exactly.
fn by_hand(what: &str, fingerprint: &Value, hand: &Sum, area: f64, counts: [u64; 3]) {
    measured(
        &format!("{what}'s volume"),
        &fingerprint["volume"],
        hand.volume,
    );
    measured(&format!("{what}'s area"), &fingerprint["area"], area);
    centred(
        &format!("{what}'s centre"),
        &fingerprint["center"],
        hand.centre(),
    );
    assert_eq!(
        fingerprint["bbox"],
        serde_json::json!([0, 0, 0, 60, 40, 50]),
        "{what}"
    );
    let got = ["faces", "edges", "vertices"].map(|k| fingerprint[k].as_u64().unwrap());
    assert_eq!(got, counts, "{what}'s faces, edges and vertices");
}

#[test]
fn the_bracket_built_in_the_app_exports_a_printable_3mf_and_a_step_the_oracle_reads_back() {
    on_lavapipe(NAME, || {
        let started = Instant::now();
        let temp = tempfile::Builder::new()
            .prefix("silico-slice-")
            .tempdir()
            .unwrap();
        let dir = std::fs::canonicalize(temp.path()).unwrap();
        let secret = dir.join("secret");
        let sessions = Arc::new(Sessions::with_cache_opener(
            KernelPool::start(),
            move |cache| Cache::with_secret(cache, &secret),
        ));
        let state = lavapipe_render_state();
        let present = silico_app::renderer::on_eframe(&state).expect("no renderer");
        let app = App::new(Arc::clone(&sessions)).presenting(Box::new(present));
        let mut h: H = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size((1280.0, 800.0))
                .with_step_dt(1.0 / 60.0)
                .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                    state.clone(),
                ))
                .build_ui_state(|ui, app: &mut App<KernelPool>| app.frame(ui), app)
        };
        h.set_render_every_step(true);
        silico_app::theme::apply(&h.ctx);
        let h = &mut h;

        // --- A new part (Command+N): "Untitled", with no file until its first save.
        new_part(h);
        settled(h);

        // --- Sketch 1, the L, on XZ: sketch x is +X, sketch y is +Z. Front (Command+1)
        // looks along +Y, face-on. Its corners, (0, 0), (60, 0), (60, 5), (5, 5), (5, 50),
        // (0, 50), and the places its dimensions are picked and placed.
        create_sketch(h, "XZ plane");
        sketch_read(h);
        let s1 = sketch_mode(h).unwrap().sketch.clone().unwrap();
        standard_view(h, Key::Num1, [0.0, 1.0, 0.0]);
        let corners = [
            [0.0, 0.0],
            [60.0, 0.0],
            [60.0, 5.0],
            [5.0, 5.0],
            [5.0, 50.0],
            [0.0, 50.0],
        ];
        let lengths: [([f64; 2], [f64; 2], &str); 4] = [
            // Line 1, along the x axis, 60; line 2, the base's end, 5; line 5, the
            // upright's top, 5; line 6, along the y axis, 50: the session's four.
            ([30.0, 0.0], [30.0, -8.0], "60"),
            ([60.0, 2.5], [67.0, 2.5], "5"),
            ([2.5, 50.0], [2.5, 57.0], "5"),
            ([0.0, 25.0], [-8.0, 25.0], "50"),
        ];
        let mut places: Vec<[f64; 2]> = corners.to_vec();
        places.extend(lengths.iter().flat_map(|(pick, place, _)| [*pick, *place]));
        let notches = clear_view(h, &places);
        eprintln!("sketch 1: the wheel out {notches} notches");
        // L, the Line tool: a click at each corner, then at the first again, which shares
        // it and ends the chain.
        h.key_press(Key::L);
        step(h);
        for corner in corners.iter().chain([&corners[0]]) {
            sketch_click(h, *corner);
        }
        read_until(h, "the L's six lines", |s| s.lines.len() == 6);
        settled(h);
        let inferred = sketch_mode(h)
            .unwrap()
            .drawable()
            .unwrap()
            .members
            .constraints
            .len();
        eprintln!(
            "sketch 1, drawn: {inferred} relations inferred: {:?}; flagged: {:?}",
            sketch_mode(h)
                .unwrap()
                .drawable()
                .unwrap()
                .members
                .constraints
                .iter()
                .map(|c| c.form.type_name())
                .collect::<Vec<_>>(),
            flagged(h)
        );
        // D, Smart Dimension: each line picked, its length placed beside it, typed.
        h.key_press(Key::D);
        step(h);
        for (k, (pick, place, value)) in lengths.iter().enumerate() {
            dimension(h, &[*pick], *place, value, inferred + k + 1);
        }
        fully_defined(h);
        let read = sketch_mode(h).unwrap().drawable().unwrap().members.clone();
        let (points, _) = geometry(&read);
        let mut spec = corners.to_vec();
        spec.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(points, spec, "sketch 1's corners as solved");
        let corner_point = read
            .points
            .iter()
            .find(|p| (p.x.mm(), p.y.mm()) == (5.0, 5.0))
            .unwrap()
            .id
            .clone();
        // G5 (its own PR, FreeCAD's *Avoid redundant*): once it lands, the L's inferred
        // relations hold nothing twice, and this sketch has no `constraints_redundant`
        // warning. Until then, recorded.
        eprintln!(
            "sketch 1, dimensioned: flagged {:?}; Modify boxes clicked into: {}",
            flagged(h),
            UNFOCUSED.with(std::cell::Cell::get)
        );
        leave_sketch(h, true);
        healthy(h, 1);
        rename(h, "Sketch1", "Bracket profile");

        // --- The pad: Length 40, Reversed (+Y: XZ's normal is −Y).
        sweep(h, "Pad", "Bracket profile");
        retype(h, "Length", "10", "40");
        tick(h, "Reversed");
        ok(h);
        healthy(h, 2);
        rename(h, "Pad", "Legs");

        // --- Sketch 2, the base's holes, on XY. Top (Command+5) looks down −Z, fitted to
        // the legs. C, the Circle tool: each centre, then a place on its rim; the second
        // centre lines up above the first, which the tool added and so woke.
        create_sketch(h, "XY plane");
        sketch_read(h);
        let s2 = sketch_mode(h).unwrap().sketch.clone().unwrap();
        standard_view(h, Key::Num5, [0.0, 0.0, -1.0]);
        let base_holes = sketch_holes(
            h,
            [[40.0, 12.0], [40.0, 28.0]],
            [1.0, 0.0],
            Holes {
                // Each circle on its top, its diameter placed up and left.
                diameters: [([40.0, 14.75], [32.0, 8.0]), ([40.0, 30.75], [32.0, 34.0])],
                // The origin and the first centre: x below the box they span, y beside it;
                // then the two centres, plumb.
                from_origin: [([20.0, -6.0], "40"), ([50.0, 6.0], "12")],
                between: ([50.0, 20.0], "16"),
            },
        );
        eprintln!(
            "sketch 2, dimensioned: flagged {base_holes:?}; Modify boxes clicked into: {}",
            UNFOCUSED.with(std::cell::Cell::get)
        );
        leave_sketch(h, true);
        healthy(h, 3);
        rename(h, "Sketch2", "Base holes");

        // --- Pocket 1: through all, Reversed (+Z: the legs stand above XY).
        sweep(h, "Pocket", "Base holes");
        choose(h, "Extent", "Through all");
        tick(h, "Reversed");
        ok(h);
        healthy(h, 4);
        rename(h, "Pocket", "Base holes");

        // --- Sketch 3, the upright's holes, on YZ: sketch x is +Y, sketch y is +Z. Right
        // (Command+4) looks along −X, Y to the right. The second centre level with the
        // first.
        create_sketch(h, "YZ plane");
        sketch_read(h);
        let s3 = sketch_mode(h).unwrap().sketch.clone().unwrap();
        standard_view(h, Key::Num4, [-1.0, 0.0, 0.0]);
        let upright_holes = sketch_holes(
            h,
            [[12.0, 35.0], [28.0, 35.0]],
            [1.0, 0.0],
            Holes {
                diameters: [([12.0, 37.75], [4.0, 30.0]), ([28.0, 37.75], [36.0, 30.0])],
                // The origin and the first centre: x above the box they span, y beside it;
                // then the two centres, level.
                from_origin: [([6.0, 44.0], "12"), ([20.0, 17.0], "35")],
                between: ([20.0, 46.0], "16"),
            },
        );
        eprintln!(
            "sketch 3, dimensioned: flagged {upright_holes:?}; Modify boxes clicked into: {}",
            UNFOCUSED.with(std::cell::Cell::get)
        );
        leave_sketch(h, true);
        healthy(h, 5);
        rename(h, "Sketch3", "Upright holes");

        // --- Pocket 2: through all, Reversed (+X).
        sweep(h, "Pocket", "Upright holes");
        choose(h, "Extent", "Through all");
        tick(h, "Reversed");
        ok(h);
        healthy(h, 6);
        rename(h, "Pocket", "Upright holes");

        // --- The fillet. Isometric (Command+7), the front, right and top toward the eye:
        // the inside corner's edge, (5, 0, 5) to (5, 40, 5), faces it. Clicked at its middle,
        // where the renderer draws (5, 20, 5), and found in lavapipe's ID buffer.
        meshes_settled(h);
        standard_view(h, Key::Num7, [-1.0, 1.0, -1.0]);
        meshes_settled(h);
        let pad = entries(h)
            .into_iter()
            .find(|(_, kind, _, _, _)| *kind == "pad")
            .unwrap()
            .0;
        let corner = format!("{pad}.pad.Edge(G {s1}.sketch.Vertex.{corner_point})");
        let screen_of = |h: &H, p: [f64; 3]| {
            let frame = frame(h).unwrap();
            let px = project(&frame.camera, frame.size_px, p).unwrap();
            let ppp = frame.pixels_per_point;
            frame.rect.min + eframe::egui::vec2(px[0] as f32 / ppp, px[1] as f32 / ppp)
        };
        // A convex edge first, the base's end's top, (60, y, 5), clicked at its middle,
        // (60, 20, 5): an edge of the pad's, from the L's corner at (60, 5). So a click at
        // a place the frame shows reaches the ID buffer's edges.
        click_at(h, screen_of(h, [60.0, 20.0, 5.0]), false);
        step_until(h, "the base's end's top edge selected", |h| {
            let names = selected_names(h);
            names.len() == 1 && names[0].starts_with(&format!("{pad}.pad.Edge("))
        });
        let convex = selected_names(h);
        // The inside corner, clicked at its middle, and the click answered. Finding N5: a
        // concave edge loses every pixel of its line to the two faces it joins, which rise
        // toward the eye from it, so the click picks a face (the one above the line, or the
        // one below it, within 2 points of it either way) and never the edge.
        let at = screen_of(h, [5.0, 20.0, 5.0]);
        assert!(
            clear(h, at, 4.0),
            "the corner at {at:?}, under {:?}",
            overlays(h)
        );
        click_at(h, at, false);
        step_until(h, "the click on the corner answered", |h| {
            selected_names(h) != convex
        });
        assert_eq!(
            selected_names(h),
            [corner.as_str()],
            "the inside corner's edge, clicked (finding N5: a concave edge can't be picked)"
        );
        modify(h, "Fillet");
        added(h);
        assert_eq!(boxed(h), [corner.as_str()]);
        retype(h, "Radius", "1", "5");
        ok(h);
        healthy(h, 7);
        rename(h, "Fillet", "Inside corner");

        // --- Saved as bracket.silico (the typed Save As: the harness has no native
        // dialogs), then Rebuild everything (Command+Shift+B): every step built anew from
        // the stored, nanometre-rounded coordinates, with no problem, so no drift.
        let path = dir.join("bracket.silico");
        save_as_of(h, "Untitled", &path);
        settled(h);
        assert!(path.is_file());
        h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::B);
        step_until(h, "rebuilt everything", |h| {
            let tab = &h.state().tabs()[0];
            tab.rebuilds_all == 1 && tab.can_rebuild_all()
        });
        healthy(h, 7);
        assert!(h.state().tabs()[0].document_problems.is_empty());
        meshes_settled(h);

        // --- Exported through the dialog: a 3MF at its default, 0.1 mm, beside the
        // document; a STEP; and a STEP to bracket.stp.
        let title = "Export bracket.silico";
        let exported = |h: &mut H, file: &Path| {
            press(h, "Export");
            step_until(h, "exported", |h| !anywhere(h, title) && file.is_file());
            settled(h);
        };
        open_export(h, title);
        assert_eq!(
            h.get_by_role_and_label(Role::TextInput, "Tolerance")
                .value()
                .as_deref(),
            Some("0.1")
        );
        exported(h, &dir.join("bracket.3mf"));
        open_export(h, title);
        press(h, "STEP");
        exported(h, &dir.join("bracket.step"));
        open_export(h, title);
        press(h, "STEP");
        set_field(h, "Path", dir.join("bracket.stp").to_str().unwrap());
        exported(h, &dir.join("bracket.stp"));
        let built = started.elapsed();

        // === The done test ===================================================================

        // --- Feature for feature: the UI's pad, pockets and fillet, in timeline order, each
        // fingerprint the recorded session's field for field, rebuilt on the same kernel.
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let reference = recorded(&dir);
        let solids = ["pad", "pocket", "fillet"];
        let ours = of_kinds(&saved, &solids);
        let theirs = of_kinds(&reference, &solids);
        assert_eq!(ours.len(), 4);
        assert_eq!(theirs.len(), 4);
        for (k, (a, b)) in ours.iter().zip(&theirs).enumerate() {
            assert_eq!(a["type"], b["type"], "solid {k}");
            assert_eq!(a["name"], b["name"], "solid {k}");
            let (fa, fb) = (&a["fingerprint"], &b["fingerprint"]);
            for field in [
                "volume", "area", "center", "bbox", "faces", "edges", "vertices",
            ] {
                assert_eq!(fa[field], fb[field], "{}'s {field}", a["name"]);
            }
            assert_eq!(fa, fb, "{}", a["name"]);
        }
        // --- And by hand. The legs: 60 · 40 · 5 + 5 · 40 · 45 = 21,000 mm³; 2 · 525 +
        // 220 · 40 = 9,850 mm²; the L-prism's 8 faces, 18 edges, 12 vertices.
        // The base's holes: two cylinders of 37.8125π less; each takes two discs of
        // π · 2.75² and adds a wall of 2π · 2.75 · 5, 12.375π more; each hole a wall, two
        // rims and a seam, two vertices on the seam.
        // The upright's: as many again. The fillet: the spandrel, (1 − π/4) · 5² · 40, more;
        // 5 · 40 from each leg's inner face less and a quarter cylinder, 100π, more, each end
        // face gaining 25(1 − π/4): 9,500 + 137π; a face for the corner's edge, its two lines
        // and two arcs for the edge, four vertices for its two.
        let mut base = legs(40.0);
        for y in [12.0, 28.0] {
            base.add(-HOLE, [40.0, y, 2.5]);
        }
        let hands: [(&Sum, f64, [u64; 3]); 4] = [
            (&legs(40.0), 9850.0, [8, 18, 12]),
            (&base, 9850.0 + 24.75 * PI, [10, 24, 16]),
            (&drilled(), 9850.0 + 49.5 * PI, [12, 30, 20]),
            (&bracket(), 9500.0 + 137.0 * PI, [13, 33, 22]),
        ];
        assert!((bracket().volume - (22000.0 - 401.25 * PI)).abs() < 1e-9);
        for (k, (hand, area, counts)) in hands.iter().enumerate() {
            for (whose, solid) in [("the app's", ours[k]), ("the session's", theirs[k])] {
                let what = format!("{whose} {}", solid["name"]);
                by_hand(&what, &solid["fingerprint"], hand, *area, *counts);
            }
        }
        // --- The sketches: each stores the session's points and radii, exactly.
        let sketches = |document: &Value| -> Vec<Value> {
            of_kinds(document, &["sketch"])
                .into_iter()
                .cloned()
                .collect()
        };
        let (our_sketches, their_sketches) = (sketches(&saved), sketches(&reference));
        assert_eq!(our_sketches.len(), 3);
        for (a, b) in our_sketches.iter().zip(&their_sketches) {
            assert_eq!(a["name"], b["name"]);
            assert_eq!(a["plane"], b["plane"], "{}", a["name"]);
            assert_eq!(stored(a), stored(b), "{}", a["name"]);
        }
        let ids: Vec<&str> = our_sketches
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [s1.as_str(), s2.as_str(), s3.as_str()]);
        // --- The fillet's edge: the pad's edge from the L's corner at (5, 5), by the
        // document's own IDs.
        let fillet = ours[3];
        assert_eq!(fillet["edges"], serde_json::json!([corner]));
        assert_eq!(fillet["radius"], 5);
        assert_eq!(fillet["base"], ours[2]["id"]);

        // --- The 3MF, as bracket.rs checks it: one object, the bracket, printable; meshed
        // at 0.1 mm, each rim 126 chords and each fillet arc 32 (OCCT's rule by hand), so
        // 8 · 126 + 2 · 33 + 2 · 5 = 1,084 vertices and, closed and of genus 4, 2 · 1,084 + 12
        // = 2,180 triangles; its box the part's exactly; and its volume the part's and its
        // chords' segments', within the 32-bit rounding.
        let archive = std::fs::read(dir.join("bracket.3mf")).unwrap();
        let meshes = model(&archive);
        assert_eq!(meshes.len(), 1, "one object, the bracket's");
        let mesh = &meshes[0];
        assert_eq!(mesh.name, "Inside corner");
        let mesh_volume = printable(mesh);
        let rim = chords(2.0 * PI, HOLE_R, TOLERANCE);
        let arc = chords(PI / 2.0, FILLET_R, TOLERANCE);
        assert_eq!((rim, arc), (126, 32));
        let any = |_: &[f64; 3]| true;
        for y in [12.0, 28.0] {
            for at in [0.0, 5.0] {
                let v = &mesh.vertices;
                assert_eq!(on_circle(v, 2, at, [40.0, y, at], HOLE_R, any), rim);
                assert_eq!(on_circle(v, 0, at, [at, y, 35.0], HOLE_R, any), rim);
            }
        }
        let inside = |v: &[f64; 3]| v[0] <= 10.0 && v[2] <= 10.0;
        for y in [0.0, 40.0] {
            assert_eq!(
                on_circle(&mesh.vertices, 1, y, [10.0, y, 10.0], FILLET_R, inside),
                arc + 1
            );
        }
        let vertices = 8 * rim + 2 * (arc + 1) + 2 * 5;
        assert_eq!(mesh.vertices.len(), vertices);
        assert_eq!(mesh.triangles.len(), 2 * vertices + 12);
        let mut low = [f64::INFINITY; 3];
        let mut high = [f64::NEG_INFINITY; 3];
        for p in &mesh.vertices {
            for i in 0..3 {
                low[i] = low[i].min(p[i]);
                high[i] = high[i].max(p[i]);
            }
        }
        assert_eq!((low, high), ([0.0; 3], [60.0, 40.0, 50.0]));
        let volume = 22000.0 - 401.25 * PI;
        let enclosed = enclosed(volume, rim, arc);
        assert!(
            (mesh_volume - enclosed).abs() <= rounding(),
            "the mesh encloses {mesh_volume} mm³; by hand {enclosed}"
        );

        // --- The STEP, read back by FreeCAD as silico-oracle's STEP tests read theirs:
        // `Part.read`, its volume and area the bracket's by hand within the measure's bound
        // and the file's digits (1e-10 relative), 13 faces, valid, its box and centre the
        // part's (1e-9 of the box's diagonal); and `Import.insert`, the product's name its
        // object's label.
        let step = dir.join("bracket.step");
        let read = silico_oracle::freecad::script(&format!(
            r#"
import json, FreeCAD, Part, Import
s = Part.read({path:?})
b = s.optimalBoundingBox(False, False)
doc = FreeCAD.newDocument("rt")
Import.insert({path:?}, doc.Name)
labels = [o.Label for o in doc.Objects if o.TypeId == "Part::Feature"]
FreeCAD.closeDocument(doc.Name)
print("SILICO_ORACLE " + json.dumps({{
    "volume": s.Volume, "area": s.Area, "faces": len(s.Faces), "solids": len(s.Solids),
    "valid": s.isValid(), "box": [b.XMin, b.YMin, b.ZMin, b.XMax, b.YMax, b.ZMax],
    "center": list(s.CenterOfGravity), "labels": labels,
}}))
"#,
            path = step.to_str().unwrap()
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        let within = |what: &str, got: &Value, hand: f64| {
            let got = got.as_f64().unwrap();
            let bound = 1e-9 * hand.abs() + 5e-7 + 1e-10 * hand.abs();
            assert!(
                (got - hand).abs() <= bound,
                "{what}: {got}, by hand {hand} ({read})"
            );
        };
        within("STEP's volume", &read["volume"], volume);
        within("STEP's area", &read["area"], 9500.0 + 137.0 * PI);
        assert_eq!(read["faces"], 13, "{read}");
        assert_eq!(read["solids"], 1, "{read}");
        assert_eq!(read["valid"], true, "{read}");
        let diagonal = (60.0f64 * 60.0 + 40.0 * 40.0 + 50.0 * 50.0).sqrt();
        let box_: [f64; 6] = serde_json::from_value(read["box"].clone()).unwrap();
        for (got, want) in box_.into_iter().zip([0.0, 0.0, 0.0, 60.0, 40.0, 50.0]) {
            assert!((got - want).abs() <= 1e-9 * diagonal, "{read}");
        }
        let centre: [f64; 3] = serde_json::from_value(read["center"].clone()).unwrap();
        for (got, want) in centre.into_iter().zip(bracket().centre()) {
            assert!(
                (got - want).abs() <= 1e-9 * diagonal + 2e-9 * want.abs(),
                "{read}"
            );
        }
        assert_eq!(
            read["labels"],
            serde_json::json!(["Inside corner"]),
            "{read}"
        );

        // --- `.stp` and `.step` write the same bytes.
        let stp = std::fs::read(dir.join("bracket.stp")).unwrap();
        assert_eq!(stp, std::fs::read(&step).unwrap());

        assert_eq!(never_waits::waits(), 0);
        eprintln!(
            "the slice: built and exported in {built:?}, checked in {:?}",
            started.elapsed()
        );
    });
}

/// Where a sketch of two holes puts them, its dimensions picked and placed.
struct Holes {
    /// Each circle's place to pick it at, and its diameter's place.
    diameters: [([f64; 2], [f64; 2]); 2],
    /// The origin and the first centre, picked, then placed at each place: the value typed.
    from_origin: [([f64; 2], &'static str); 2],
    /// The two centres, picked, then placed: the value typed.
    between: ([f64; 2], &'static str),
}

/// The open sketch's two holes, each 5.5 across, about `centres`, drawn with the Circle
/// tool (the centre, then a place on the rim `along` from it) and dimensioned as `holes`
/// says, to fully defined; its geometry checked against the spec's, and what the solve's
/// diagnosis flags, returned to record.
fn sketch_holes(
    h: &mut H,
    centres: [[f64; 2]; 2],
    along: [f64; 2],
    holes: Holes,
) -> Vec<(String, ConstraintState)> {
    let rims = centres.map(|c| [c[0] + HOLE_R * along[0], c[1] + HOLE_R * along[1]]);
    let mut places: Vec<[f64; 2]> = vec![[0.0, 0.0]];
    places.extend(centres);
    places.extend(rims);
    places.extend(holes.diameters.iter().flat_map(|(a, b)| [*a, *b]));
    places.extend(holes.from_origin.iter().map(|(p, _)| *p));
    places.push(holes.between.0);
    let notches = clear_view(h, &places);
    eprintln!("the wheel out {notches} notches");
    h.key_press(Key::C);
    step(h);
    for (k, (centre, rim)) in centres.iter().zip(&rims).enumerate() {
        sketch_click(h, *centre);
        sketch_click(h, *rim);
        read_until(h, "the circle", |s| s.circles.len() == k + 1);
        settled(h);
    }
    let inferred = sketch_mode(h)
        .unwrap()
        .drawable()
        .unwrap()
        .members
        .constraints
        .len();
    eprintln!(
        "drawn: {:?}",
        sketch_mode(h)
            .unwrap()
            .drawable()
            .unwrap()
            .members
            .constraints
            .iter()
            .map(|c| c.form.type_name())
            .collect::<Vec<_>>()
    );
    h.key_press(Key::D);
    step(h);
    let mut count = inferred;
    for (pick, place) in holes.diameters {
        count += 1;
        dimension(h, &[pick], place, "5.5", count);
    }
    for (place, value) in holes.from_origin {
        count += 1;
        dimension(h, &[[0.0, 0.0], centres[0]], place, value, count);
    }
    count += 1;
    dimension(h, &centres, holes.between.0, holes.between.1, count);
    fully_defined(h);
    let read = sketch_mode(h).unwrap().drawable().unwrap().members.clone();
    let (points, circles) = geometry(&read);
    let mut spec = centres.to_vec();
    spec.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(points, spec, "the centres as solved");
    let spec_circles: Vec<([f64; 2], f64)> = spec.iter().map(|c| (*c, HOLE_R)).collect();
    assert_eq!(circles, spec_circles, "the circles as solved");
    flagged(h)
}
