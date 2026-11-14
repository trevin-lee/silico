//! Hostile input for the viewport (M5.6): the camera controls given non-finite, degenerate,
//! and extreme numbers; the `meshes` read cycle at the edges of `serves`, refused, and
//! interrupted (a tab switched, a session closed, a body renumbered, a revision changed,
//! all mid-read); a display past 4,096 pixels; pointer events out of order; and the hover
//! linking between the timeline and the canvas (a feature tinted, an item ringed) through a
//! feature removed or renumbered on disk mid-hover, a hover in another tab, a stale or
//! nameless pick, and many features, each frame held to the renderer's rules. Driven as
//! the app's own tests drive it, through `egui_kittest` on the fake kernel, every frame on
//! a thread marked so that a wait on a command fails. Expected values by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/ui.rs"]
mod ui;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Event, Modifiers, PointerButton, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use silico_app::App;
use silico_app::navigate;
use silico_app::renderer::{Change, FrameData, Picking, Present, ViewRenderer};
use silico_app::viewport::{Navigation, read_size};
use silico_core::cache::Cache;
use silico_core::command::Sessions;
use silico_core::document::FeatureId;
use silico_core::document::json;
use silico_core::fake::{FakeKernel, FakeSketches, never_waits};
use silico_core::kernel::Op;
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Camera, MAX_RENDER_SIDE, Mesh, Projection};
use tempfile::TempDir;
use ui::{
    AXIS_X, AXIS_Y, AXIS_Z, FAKE_WAIT as WAIT, TRIAD_DOT, advance, body_of, button_event,
    hand_clock, hover_item, looks_from, meshes_settled, meshes_settled_in, repaint_delay, step,
    step_until, tessellated, the_viewport, triad_origin, view, view_label, view_mut,
};

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

/// A 50 mm cube at the origin and a 1 m cube 2 m along x: bodies 1 and 2.
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

type Fake = Arc<FakeKernel>;
type AppHarness = Harness<'static, App<Fake>>;

struct Fixture {
    dir: TempDir,
    kernel: Fake,
    sessions: Arc<Sessions<Fake>>,
}

fn fixture() -> Fixture {
    fixture_with(false)
}

/// [`fixture`], each file compared on disk with no interval, so one written under a
/// session with nothing unsaved is reloaded by it on its own (as an agent's commit is).
fn watching() -> Fixture {
    fixture_with(true)
}

fn fixture_with(watch: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let solver = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let sessions = if watch {
        let now: silico_core::disk::Clock = Arc::new(std::time::Instant::now);
        sessions
            .watching_disk(Duration::ZERO, Arc::new(ui::plain_read), now)
            .unwrap()
    } else {
        sessions
    };
    Fixture {
        dir,
        kernel,
        sessions: Arc::new(sessions),
    }
}

/// What the fake renderer was told.
#[derive(Default)]
struct Told {
    uploads: Vec<(u32, Arc<Mesh>)>,
    forgets: Vec<u32>,
    frames: Vec<FrameData>,
    /// Frames presented with no tab shown.
    empty: usize,
    /// The last frame's picking, answered the frame after as a renderer answers it: `hit`,
    /// or with none, the background's (the hover the selection asks as the pointer moves,
    /// M5.7).
    picking: Option<Picking>,
    /// What every pick finds from now on, if anything.
    hit: Option<silico_app::renderer::Pick>,
    /// The meshes the renderer holds, by body, as it was told.
    held: std::collections::BTreeMap<u32, Arc<Mesh>>,
    /// Why silico-render would have refused each frame it would have (`Frame::check`).
    refused: Vec<String>,
}

type Record = Arc<Mutex<Told>>;

/// The pane's height in pixels, as its last frame had it.
fn rows(record: &Record) -> u32 {
    record.lock().unwrap().frames.last().unwrap().size_px[1]
}

/// The least an orthographic view of bodies reaching `reach` mm from its target may be in
/// the pane, as the app takes it: the renderer's at the pane's height in pixels, `rows ×
/// reach / 2^19`, and a hundredth more.
fn least_in(record: &Record, reach: f64) -> f64 {
    f64::from(rows(record)) * reach / 524_288.0 * 1.01
}

struct FakeRenderer(Record);

impl ViewRenderer for FakeRenderer {
    type Failure = String;
    fn upload(&mut self, body: u32, mesh: &Arc<Mesh>) -> Result<(), String> {
        let mut told = self.0.lock().unwrap();
        told.uploads.push((body, Arc::clone(mesh)));
        told.held.insert(body, Arc::clone(mesh));
        Ok(())
    }
    fn forget(&mut self, body: u32) {
        let mut told = self.0.lock().unwrap();
        told.forgets.push(body);
        told.held.remove(&body);
    }
    fn prepare(
        &mut self,
        _: &silico_app::renderer::Frame<'_>,
    ) -> Result<eframe::wgpu::CommandBuffer, String> {
        panic!("no GPU in these tests")
    }
    fn composite(&self, _: &mut eframe::wgpu::RenderPass<'_>) {}
    fn take_picking(&mut self) -> Option<Picking> {
        None
    }
}

struct Recorder(FakeRenderer);

impl Present for Recorder {
    fn present(
        &mut self,
        _: &egui::Painter,
        _: egui::Rect,
        changes: Vec<Change>,
        frame: Option<FrameData>,
    ) {
        assert!(silico_app::renderer::apply(&mut self.0, changes).is_empty());
        let mut told = self.0.0.lock().unwrap();
        match frame {
            Some(frame) => {
                assert!(frame.bodies.windows(2).all(|pair| pair[0] < pair[1]));
                if let Some(rect) = frame.picking {
                    // What it finds at the square's centre, as a renderer answers the pixel
                    // nearest it.
                    let at = |min: i32, side: u32| {
                        u32::try_from(i64::from(min) + i64::from(side / 2)).unwrap()
                    };
                    let centre = [at(rect.min[0], rect.size[0]), at(rect.min[1], rect.size[1])];
                    told.picking = Some(Picking {
                        frame: frame.serial,
                        rect,
                        hit: told.hit.map(|hit| silico_app::renderer::Pick {
                            pixel: centre,
                            ..hit
                        }),
                        edge: None,
                    });
                }
                // silico-render's own check of the frame's values against the meshes held,
                // the one its `prepare` refuses by.
                let refused = frame
                    .frame()
                    .check(|body| told.held.get(&body).map(|mesh| &**mesh))
                    .err();
                if let Some(why) = refused {
                    told.refused
                        .push(format!("{}: {}", why.parameter, why.message));
                }
                told.frames.push(frame);
            }
            None => told.empty += 1,
        }
    }
    fn picked(&mut self) -> Option<Picking> {
        self.0.0.lock().unwrap().picking.take()
    }
    fn refused(&mut self) -> std::collections::BTreeSet<u32> {
        std::collections::BTreeSet::new()
    }
}

impl Fixture {
    fn file(&self, name: &str, text: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path.to_str().unwrap().to_owned()
    }

    /// The app, `size` points, drawn through the fake renderer, with `name` open.
    fn drawn(&self, name: &str, text: &str, size: (f32, f32)) -> (AppHarness, Record) {
        let record = Record::default();
        let app = App::new(Arc::clone(&self.sessions))
            .presenting(Box::new(Recorder(FakeRenderer(Arc::clone(&record)))));
        let mut h = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size(size)
                .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
        };
        silico_app::theme::apply(&h.ctx);
        let path = self.file(name, text);
        h.state_mut().open(&path, false);
        (h, record)
    }
}

fn view_height(camera: &Camera) -> f64 {
    match camera.projection {
        Projection::Orthographic { view_height } => view_height,
        Projection::Perspective { .. } => panic!("orthographic"),
    }
}

fn distance(camera: &Camera) -> f64 {
    let d = [
        camera.eye[0] - camera.target[0],
        camera.eye[1] - camera.target[1],
        camera.eye[2] - camera.target[2],
    ];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn front(view_height: f64) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height },
        eye: [0.0, -100.0, 0.0],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    }
}

fn perspective(distance: f64) -> Camera {
    Camera {
        projection: Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: [0.0, -distance, 0.0],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    }
}

// ---- The camera's maths ----

#[test]
fn non_finite_drags_wheels_and_cursors_leave_the_view_as_it_was() {
    for camera in [front(80.0), perspective(100.0)] {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(navigate::orbit(&camera, bad, 0.0), None, "{bad}");
            assert_eq!(navigate::orbit(&camera, 0.0, bad), None, "{bad}");
            assert_eq!(navigate::pan(&camera, bad, 0.0, 400.0), None, "{bad}");
            assert_eq!(navigate::pan(&camera, 0.0, bad, 400.0), None, "{bad}");
            assert_eq!(navigate::zoom(&camera, bad, [0.0, 0.0], 400.0), None);
            assert_eq!(navigate::zoom(&camera, 200.0, [bad, 0.0], 400.0), None);
            assert_eq!(navigate::pan(&camera, 1.0, 1.0, bad), None);
            assert_eq!(navigate::zoom(&camera, 200.0, [0.0, 0.0], bad), None);
        }
    }
}

#[test]
fn a_viewport_of_no_height_pans_and_zooms_nothing_and_fits_nothing_wrong() {
    // No points high: no scale to pan or zoom by. A fit to a viewport of no width or height
    // is refused or a camera that passes its check, never a NaN one.
    let camera = front(80.0);
    for height in [0.0, -1.0] {
        assert_eq!(navigate::pan(&camera, 10.0, 10.0, height), None);
        assert_eq!(navigate::zoom(&camera, 200.0, [5.0, 5.0], height), None);
    }
    let cube = [[0.0; 3], [20.0; 3]];
    for (width, height) in [(0, 0), (0, 400), (400, 0), (1, 1), (1, 4096), (4096, 1)] {
        if let Some(fitted) = navigate::fit(&camera, cube, width, height) {
            assert!(
                fitted.check(width.max(1), height.max(1)).is_ok(),
                "{width} by {height}"
            );
        }
        let named = navigate::named(navigate::HOME, cube, width, height).unwrap();
        assert!(
            named.check(width.max(1), height.max(1)).is_ok(),
            "{width} by {height}"
        );
    }
    // One pixel high, one point a pixel: a pan of a point moves the whole view's height.
    let panned = navigate::pan(&camera, 0.0, 1.0, 1.0).unwrap();
    assert_eq!(panned.target, [0.0, 0.0, 80.0]);
}

#[test]
fn zooming_in_and_out_without_end_stops_at_what_a_renderer_draws() {
    // 80 mm halved at each step stops once a halving would pass 1e-6 mm, and doubled stops
    // once one would pass 1e12 mm; perspective, once the eye would come within 1e-6 mm of
    // the target, or go past 1e12 mm. Every camera on the way passes its check.
    for start in [front(80.0), perspective(100.0)] {
        for scroll in [200.0, -200.0] {
            let mut camera = start.clone();
            let mut steps = 0;
            while let Some(next) = navigate::zoom(&camera, scroll, [37.0, -11.0], 400.0) {
                assert!(next.frame().is_ok());
                camera = next;
                steps += 1;
                assert!(steps < 200, "zoomed {steps} times without stopping");
            }
            match camera.projection {
                Projection::Orthographic { view_height } if scroll > 0.0 => {
                    // 80 · 2⁻²⁶ is the last halving at or over 1e-6.
                    assert_eq!(steps, 26);
                    assert_eq!(view_height, 80.0 / 67_108_864.0);
                }
                Projection::Orthographic { view_height } => {
                    assert!(view_height <= 1e12 && view_height * 2.0 > 1e12);
                }
                Projection::Perspective { .. } if scroll > 0.0 => {
                    assert!(distance(&camera) >= 1e-6 && distance(&camera) / 2.0 < 1e-6);
                }
                Projection::Perspective { .. } => {
                    assert!(camera.eye.iter().all(|c| c.abs() <= 1e12));
                }
            }
        }
    }
}

#[test]
fn an_orbit_over_the_poles_and_round_comes_back_where_it_began() {
    // 720 drags of a point down: a whole turn about the view's right, over the top and
    // under the bottom. Each step passes its check, keeps the eye's distance, and keeps the
    // up across the view; the last is the first, to 1e-9 of the distance.
    for start in [front(80.0), perspective(100.0)] {
        let mut camera = start.clone();
        for step in 0..720 {
            camera = navigate::orbit(&camera, 0.0, 1.0).unwrap();
            let frame = camera.frame().unwrap();
            assert!((distance(&camera) - 100.0).abs() < 1e-9, "step {step}");
            let along = frame.up[0] * frame.back[0]
                + frame.up[1] * frame.back[1]
                + frame.up[2] * frame.back[2];
            assert!(along.abs() < 1e-12, "step {step}");
        }
        for k in 0..3 {
            assert!(
                (camera.eye[k] - start.eye[k]).abs() < 1e-9 * 100.0,
                "{camera:?}"
            );
            assert!((camera.up[k] - start.up[k]).abs() < 1e-9, "{camera:?}");
        }
        // Straight down onto the target: one quarter turn up from the front, 180 points.
        let over = navigate::orbit(&start, 0.0, 180.0).unwrap();
        assert!(over.frame().is_ok());
        assert!((over.eye[2] - 100.0).abs() < 1e-12 && over.eye[0].abs() < 1e-12);
        // Turned about z there, the view spins about its own axis, still valid.
        let spun = navigate::orbit(&over, 90.0, 0.0).unwrap();
        assert!(spun.frame().is_ok());
        assert!((spun.eye[2] - 100.0).abs() < 1e-12);
    }
}

#[test]
fn a_pan_a_metre_out_and_back_comes_back_to_a_micrometre() {
    // 1e6 mm out on every axis, 0.2 mm a point: 1,000 pans of a point each way, and a turn
    // and back, leave the target within 1e-6 mm of where it was.
    let far = Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: [1e6, 1e6 - 100.0, 1e6],
        target: [1e6; 3],
        up: [0.0, 0.0, 1.0],
    };
    let mut camera = far.clone();
    for _ in 0..1000 {
        camera = navigate::pan(&camera, 1.0, -1.0, 400.0).unwrap();
    }
    assert!((camera.target[0] - (1e6 - 200.0)).abs() < 1e-6);
    assert!((camera.target[2] - (1e6 - 200.0)).abs() < 1e-6);
    for _ in 0..1000 {
        camera = navigate::pan(&camera, -1.0, 1.0, 400.0).unwrap();
    }
    for k in 0..3 {
        assert!(
            (camera.target[k] - far.target[k]).abs() < 1e-6,
            "{camera:?}"
        );
        assert!((camera.eye[k] - far.eye[k]).abs() < 1e-6, "{camera:?}");
    }
}

#[test]
fn a_perspective_eye_a_micrometre_from_its_target_still_pans_orbits_and_zooms_out() {
    // 4e-6 mm away: a halving is taken, a second (1e-6, at the least) too, a third refused;
    // a pan and an orbit there keep a valid camera; zooming out again works.
    let near = perspective(4e-6);
    let once = navigate::zoom(&near, 200.0, [0.0, 0.0], 400.0).unwrap();
    assert!((distance(&once) - 2e-6).abs() < 1e-18);
    let twice = navigate::zoom(&once, 200.0, [0.0, 0.0], 400.0).unwrap();
    assert!(navigate::zoom(&twice, 200.0, [0.0, 0.0], 400.0).is_none());
    assert!(
        navigate::pan(&twice, 50.0, 50.0, 400.0)
            .unwrap()
            .frame()
            .is_ok()
    );
    assert!(navigate::orbit(&twice, 50.0, 50.0).unwrap().frame().is_ok());
    let out = navigate::zoom(&twice, -800.0, [0.0, 0.0], 400.0).unwrap();
    assert!((distance(&out) - 1.6e-5).abs() < 1e-17);
}

#[test]
fn a_read_past_4096_pixels_keeps_the_viewports_shape() {
    // A 1,440 by 1,067 point viewport at 3× is 4,320 by 3,200 pixels: scaled to fit 4,096
    // on its longer side it's 4,096 by 3,034 (3,200 · 4,096 / 4,320 = 3,034.07). Clamped
    // side by side it would be 4,096 by 3,200, and Home's fit, which takes the read's
    // width over its height, would fit another shape than the one shown.
    // BREAKS (fixed): each side was clamped alone, so the read's aspect wasn't the view's.
    assert_eq!(read_size([4320, 3200]), [4096, 3034]);
    assert_eq!(read_size([1000, 6000]), [683, 4096]);
    assert_eq!(read_size([8192, 1]), [4096, 1]);
    assert_eq!(read_size([0, 0]), [1, 1]);
    assert_eq!(read_size([u32::MAX, u32::MAX]), [4096, 4096]);
    for size in [[3840, 2160], [2560, 1600], [4096, 4096]] {
        assert_eq!(read_size(size), size);
    }
    for size in [[4320_u32, 3200_u32], [5000, 17], [17, 5000], [9000, 9001]] {
        let read = read_size(size);
        assert!(read.iter().all(|side| (1..=MAX_RENDER_SIDE).contains(side)));
        let (shown, asked) = (
            f64::from(size[0]) / f64::from(size[1]),
            f64::from(read[0]) / f64::from(read[1]),
        );
        // Within the rounding of the shorter side to a whole pixel.
        let shorter = f64::from(read[0].min(read[1]));
        assert!(
            (asked / shown - 1.0).abs() <= 0.5 / shorter + 1e-12,
            "{size:?}"
        );
    }
}

// ---- The read cycle ----

/// Reads made over `frames` frames once settled.
fn reads_over(h: &mut AppHarness, frames: usize) -> u64 {
    let before = view(h).reads();
    for _ in 0..frames {
        step(h);
    }
    step_until(h, "no read out", |h| !view(h).reading());
    view(h).reads() - before
}

#[test]
fn a_view_at_the_edges_of_serves_reads_once_and_then_never_over_a_hundred_frames() {
    let f = fixture();
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    assert_eq!(reads_over(&mut h, 100), 0);
    let height = f64::from(view(&h).frame().unwrap().size_px[1]);
    // Orthographic, the half pixel exactly a power of two, 2⁻⁴ and 2⁻⁵: each served by its
    // own level, [2⁻⁴, 2⁻³) and [2⁻⁵, 2⁻⁴), so one read each and no more. Exactly at the
    // large cube's floor's range's end, 2 · 2⁻³ = 0.25: the next level, 2⁻², serves it.
    for half in [0.0625, 0.03125, 0.25, 0.25 * (1.0 - f64::EPSILON)] {
        view_mut(&mut h).set_camera(Camera {
            projection: Projection::Orthographic {
                view_height: 2.0 * half * height,
            },
            eye: [25.0, -500.0, 25.0],
            target: [25.0, 25.0, 25.0],
            up: [0.0, 0.0, 1.0],
        });
        meshes_settled(&mut h);
        assert_eq!(reads_over(&mut h, 100), 0, "half pixel {half}");
    }
    // Perspective: the eye in the large cube's box's plane (its depth there 0, so its level
    // its floor), and one far off; each read once, then none.
    for eye in [
        [2500.0, -1000.0, 500.0],
        [2500.0, -1e5, 500.0],
        [25.0, -0.5, 25.0],
    ] {
        view_mut(&mut h).set_camera(Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            eye,
            target: [25.0, 25.0, 25.0],
            up: [0.0, 0.0, 1.0],
        });
        meshes_settled(&mut h);
        assert_eq!(reads_over(&mut h, 100), 0, "eye {eye:?}");
    }
    assert_eq!(never_waits::waits(), 0);
}

/// Frames until the core's waker has told of everything since `before` wakes: a read's
/// answer wakes the app once its reply is sent, which may be after the frame that took it,
/// and its frame asked at once would make a measured delay ZERO.
fn woken(h: &mut AppHarness, before: u64) {
    step_until(h, "the read's wake", |h| h.state().wakes() > before);
}

impl Fixture {
    /// [`Fixture::drawn`], its viewport's time `clock`'s.
    fn drawn_clocked(
        &self,
        name: &str,
        text: &str,
        clock: silico_core::disk::Clock,
    ) -> (AppHarness, Record) {
        let record = Record::default();
        let app = App::new(Arc::clone(&self.sessions))
            .presenting_clocked(Box::new(Recorder(FakeRenderer(Arc::clone(&record)))), clock);
        // A sixtieth of a second a frame: egui asks for a frame at once for any delay
        // shorter than a frame, which kittest's default quarter second would hide.
        let mut h = {
            let _marked = never_waits::marked();
            Harness::builder()
                .with_size((1280.0, 800.0))
                .with_step_dt(1.0 / 60.0)
                .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
        };
        silico_app::theme::apply(&h.ctx);
        let path = self.file(name, text);
        h.state_mut().open(&path, false);
        (h, record)
    }
}

/// The kernel refusing to mesh while the flag is set.
fn failing_kernel(f: &Fixture) -> Arc<AtomicBool> {
    let failing = Arc::new(AtomicBool::new(false));
    let refusing = Arc::clone(&failing);
    f.kernel.refuse(move |op| {
        (matches!(op, Op::Tessellate { .. }) && refusing.load(Ordering::SeqCst))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the worker crashed meshing"))
    });
    failing
}

/// The small cube from the front at a half pixel of 0.04 mm.
fn the_small_cube_at_0_04(h: &AppHarness) -> Camera {
    let height = f64::from(view(h).frame().unwrap().size_px[1]);
    Camera {
        projection: Projection::Orthographic {
            view_height: 0.08 * height,
        },
        eye: [25.0, -500.0, 25.0],
        target: [25.0, 25.0, 25.0],
        up: [0.0, 0.0, 1.0],
    }
}

#[test]
fn a_read_refused_while_the_kernel_fails_is_asked_again_on_its_backoff_and_a_new_revision_at_once()
{
    // The time the viewport waits by is the test's: frozen, then moved by hand.
    let f = fixture();
    let failing = failing_kernel(&f);
    let (time, clock) = hand_clock();
    let (mut h, _record) = f.drawn_clocked("boxes.silico", TWO_BOXES, clock);
    meshes_settled(&mut h);
    failing.store(true, Ordering::SeqCst);
    let reads = view(&h).reads();
    let wakes = h.state().wakes();
    let zoomed = the_small_cube_at_0_04(&h);
    view_mut(&mut h).set_camera(zoomed);
    step_until(&mut h, "the refused read", |h| {
        view(h).reads() > reads && !view(h).reading()
    });
    woken(&mut h, wakes);
    assert_ne!(view(&h).bodies()[&1].level, 0.03125);
    let message = h.state().tabs()[0].message.clone().unwrap_or_default();
    assert!(
        message.starts_with("The view's meshes couldn't be read: the worker crashed meshing"),
        "{message}"
    );
    // Settled: a frame is asked for in exactly 250 ms, and none is read meanwhile however
    // many frames come.
    for _ in 0..3 {
        step(&mut h);
    }
    // About 250 ms: egui takes a frame off what's asked.
    let about = |h: &AppHarness, wait: u64| {
        repaint_delay(h).is_some_and(|delay| {
            delay <= Duration::from_millis(wait) && delay > Duration::from_millis(wait - 20)
        })
    };
    assert!(about(&h, 250), "{:?}", repaint_delay(&h));
    assert_eq!(reads_over(&mut h, 100), 0);
    // A millisecond short: still none (egui asks for a frame at once for a wait shorter
    // than a frame, so the delay isn't asserted here).
    advance(&time, Duration::from_millis(249));
    assert_eq!(reads_over(&mut h, 10), 0);
    // Due: read once, refused again, and the next in 500 ms.
    let wakes = h.state().wakes();
    advance(&time, Duration::from_millis(1));
    assert_eq!(reads_over(&mut h, 10), 1);
    woken(&mut h, wakes);
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(about(&h, 500), "{:?}", repaint_delay(&h));
    // Said once for the revision: the message is the same one, not said again.
    h.state_mut().tabs_mut()[0].message = None;
    advance(&time, Duration::from_millis(500));
    assert_eq!(reads_over(&mut h, 10), 1);
    assert_eq!(h.state().tabs()[0].message, None);
    // A new revision during the backoff (the next is a second away): read at once, the
    // time not moved, and the kernel mended.
    failing.store(false, Ordering::SeqCst);
    let id = h.state().tabs()[0].id;
    let revision = f.sessions.status(&id).session.unwrap().revision;
    f.sessions
        .set_field(
            &id,
            revision,
            FeatureId::new("k7f2qx3m").unwrap(),
            "height".to_owned(),
            json::read(b"60").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the new revision read", |h| {
        view(h)
            .bodies()
            .get(&1)
            .is_some_and(|body| body.level == 0.03125 && body.bounds.unwrap()[1][2] == 60.0)
    });
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

/// The tab's message, once the tab is open.
fn message(h: &AppHarness) -> Option<String> {
    h.state().tabs().first().and_then(|tab| tab.message.clone())
}

#[test]
fn a_refusal_said_is_cleared_by_the_next_answer_but_a_message_said_since_is_kept() {
    // The first read, of Home's view, refused while the kernel fails: said. Mended, the
    // retry due in 250 ms, Home's again, answers, and what the refusal said is cleared. A
    // new revision refused again is said again; then something else is said on the tab (a
    // refused edit, say), and the retry's answer leaves that be: it clears only its own
    // refusal.
    let f = fixture();
    let failing = failing_kernel(&f);
    failing.store(true, Ordering::SeqCst);
    let (time, clock) = hand_clock();
    let (mut h, _record) = f.drawn_clocked("boxes.silico", TWO_BOXES, clock);
    step_until(&mut h, "the first read's refusal said", |h| {
        message(h).is_some()
    });
    // Said by that read itself, Home's: its refusal isn't Home's want of a box.
    assert_eq!(view(&h).reads(), 1);
    let said = message(&h).unwrap();
    assert!(
        said.starts_with("The view's meshes couldn't be read: the worker crashed meshing"),
        "{said}"
    );
    failing.store(false, Ordering::SeqCst);
    advance(&time, Duration::from_millis(250));
    meshes_settled(&mut h);
    assert_eq!(message(&h), None);
    // The retry was of Home's view too, and its camera taken: onto the centre of the two
    // boxes' joined box, (0, 0, 0) to (3000, 1000, 1000).
    assert_eq!(view(&h).reads(), 2);
    assert_eq!(view(&h).camera().target, [1500.0, 500.0, 500.0]);
    // A new revision, refused.
    failing.store(true, Ordering::SeqCst);
    let id = h.state().tabs()[0].id;
    let revision = f.sessions.status(&id).session.unwrap().revision;
    f.sessions
        .set_field(
            &id,
            revision,
            FeatureId::new("k7f2qx3m").unwrap(),
            "height".to_owned(),
            json::read(b"60").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the new revision's refusal said", |h| {
        message(h).is_some()
    });
    let again = message(&h).unwrap();
    assert!(
        again.starts_with("The view's meshes couldn't be read: the worker crashed meshing"),
        "{again}"
    );
    let other = "Something else the tab says".to_owned();
    h.state_mut().tabs_mut()[0].message = Some(other.clone());
    failing.store(false, Ordering::SeqCst);
    advance(&time, Duration::from_millis(250));
    step_until(&mut h, "the new revision read", |h| {
        view(h)
            .bodies()
            .get(&1)
            .is_some_and(|body| body.bounds.unwrap()[1][2] == 60.0)
    });
    meshes_settled(&mut h);
    assert_eq!(message(&h), Some(other));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_due_retry_with_no_read_wanted_asks_for_no_frame_at_the_displays_rate() {
    // Refused, then the view set back where the last answer was: when the retry falls due,
    // no read is wanted, and the frame asked for is the idle one, never one at once.
    let f = fixture();
    let failing = failing_kernel(&f);
    let (time, clock) = hand_clock();
    let (mut h, _record) = f.drawn_clocked("boxes.silico", TWO_BOXES, clock);
    meshes_settled(&mut h);
    let settled = view(&h).camera().clone();
    failing.store(true, Ordering::SeqCst);
    let reads = view(&h).reads();
    let wakes = h.state().wakes();
    let zoomed = the_small_cube_at_0_04(&h);
    view_mut(&mut h).set_camera(zoomed);
    step_until(&mut h, "the refused read", |h| {
        view(h).reads() > reads && !view(h).reading()
    });
    woken(&mut h, wakes);
    view_mut(&mut h).set_camera(settled);
    advance(&time, Duration::from_millis(300));
    // The first frame sees the view move and asks for the next; egui repaints once more
    // after a frame asked at once; the third is the first of the idle ones.
    for _ in 0..3 {
        step(&mut h);
    }
    let idle = |delay: Option<Duration>| delay.is_some_and(|delay| delay > Duration::from_secs(1));
    assert!(idle(repaint_delay(&h)), "{:?}", repaint_delay(&h));
    for _ in 0..5 {
        step(&mut h);
        assert!(idle(repaint_delay(&h)), "{:?}", repaint_delay(&h));
    }
    assert_eq!(view(&h).reads(), reads + 1);
    // That run of refusals ended: zoomed again and refused, the next waits 250 ms, the
    // shortest, not the 500 a second refusal in a row would.
    let wakes = h.state().wakes();
    let zoomed = the_small_cube_at_0_04(&h);
    view_mut(&mut h).set_camera(zoomed);
    step_until(&mut h, "the second refused read", |h| {
        view(h).reads() > reads + 1 && !view(h).reading()
    });
    woken(&mut h, wakes);
    for _ in 0..3 {
        step(&mut h);
    }
    let delay = repaint_delay(&h).unwrap_or_default();
    assert!(
        delay <= Duration::from_millis(250) && delay > Duration::from_millis(230),
        "{delay:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tab_switched_mid_read_takes_its_answer_when_shown_again_and_the_renderer_follows() {
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    let first = Arc::clone(&view(&h).bodies()[&1].mesh);
    // Its next read held; the second document opened and shown meanwhile.
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let height = f64::from(view(&h).frame().unwrap().size_px[1]);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic {
            view_height: 0.01 * height,
        },
        eye: [20.0, -500.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    });
    step_until(&mut h, "a's read held", |h| view(h).reading());
    let path = f.file("b.silico", TWO_BOXES);
    h.state_mut().open(&path, false);
    step_until(&mut h, "b shown", |h| h.state().tabs().len() == 2);
    hold.release();
    meshes_settled_in(&mut h, 1);
    // a's body forgotten, b's two uploaded, as b is shown.
    {
        let told = record.lock().unwrap();
        assert_eq!(told.forgets, [1]);
        let bodies: Vec<u32> = told.uploads.iter().map(|(n, _)| *n).collect();
        assert_eq!(bodies, [1, 1, 2]);
        assert_eq!(told.frames.last().unwrap().bodies, [1, 2]);
    }
    // a shown again: its held read's answer taken, its finer mesh uploaded.
    h.get_by_label("a.silico").click();
    meshes_settled(&mut h);
    let mesh = Arc::clone(&view(&h).bodies()[&1].mesh);
    assert!(!Arc::ptr_eq(&mesh, &first));
    let told = record.lock().unwrap();
    assert!(Arc::ptr_eq(&told.uploads.last().unwrap().1, &mesh));
    assert_eq!(told.frames.last().unwrap().bodies, [1]);
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_session_closed_mid_read_leaves_nothing_held_and_frames_go_on() {
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let height = f64::from(view(&h).frame().unwrap().size_px[1]);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic {
            view_height: 0.01 * height,
        },
        eye: [20.0, -500.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    });
    step_until(&mut h, "the read held", |h| view(h).reading());
    h.get_by_label("Close a.silico").click();
    step_until(&mut h, "the tab closed", |h| h.state().tabs().is_empty());
    hold.release();
    let frames = h.state().frames();
    for _ in 0..20 {
        step(&mut h);
    }
    assert!(h.state().frames() >= frames + 20);
    let told = record.lock().unwrap();
    assert_eq!(told.forgets, [1]);
    assert!(told.empty >= 20);
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_body_renumbered_keeps_its_mesh_and_is_uploaded_once_under_its_new_number() {
    // The small cube removed: the large one is body 1 now, its key and level unchanged, so
    // it's answered held: no mesh made, its mesh uploaded as body 1, and body 2 forgotten.
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    let large = Arc::clone(&view(&h).bodies()[&2].mesh);
    let made = tessellated(&f.kernel);
    let uploads = record.lock().unwrap().uploads.len();
    let id = h.state().tabs()[0].id;
    let revision = f.sessions.status(&id).session.unwrap().revision;
    f.sessions
        .remove_feature(&id, revision, FeatureId::new("k7f2qx3m").unwrap())
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the removal read", |h| {
        view(h).bodies().len() == 1 && !view(h).reading()
    });
    step(&mut h);
    assert!(Arc::ptr_eq(&view(&h).bodies()[&1].mesh, &large));
    assert_eq!(tessellated(&f.kernel), made);
    let told = record.lock().unwrap();
    assert_eq!(told.forgets, [2]);
    assert_eq!(told.uploads.len(), uploads + 1);
    let (body, mesh) = told.uploads.last().unwrap();
    assert_eq!(*body, 1);
    assert!(Arc::ptr_eq(mesh, &large));
    assert_eq!(told.frames.last().unwrap().bodies, [1]);
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_revision_published_mid_read_is_read_after_it_and_the_last_answer_is_the_newest() {
    let f = fixture();
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    let id = h.state().tabs()[0].id;
    let set = |f: &Fixture, length: &[u8]| {
        let revision = f.sessions.status(&id).session.unwrap().revision;
        f.sessions
            .set_field(
                &id,
                revision,
                FeatureId::new("k7f2qx3m").unwrap(),
                "length".to_owned(),
                json::read(length).unwrap(),
            )
            .wait()
            .result
            .unwrap();
    };
    let reads = view(&h).reads();
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    set(&f, b"60");
    step_until(&mut h, "the first edit's read sent", |h| view(h).reading());
    // Held at its tessellation, so it has taken the first edit's model: a published read
    // takes the newest model as its thread starts, not as it's sent, and one that started
    // only after the second edit published would answer that one's at once (one read, not
    // two: `a_read_answered_of_a_newer_revision_than_asked_is_not_asked_again`).
    assert!(f.kernel.wait_running(1, WAIT));
    set(&f, b"70");
    let newest = f.sessions.status(&id).session.unwrap().revision;
    step_until(&mut h, "the second edit published", |h| {
        h.state().tabs()[0].published == Some(newest) && view(h).reading()
    });
    hold.release();
    meshes_settled(&mut h);
    // The held read, then one of the newest revision: two, and the bodies its.
    assert_eq!(view(&h).reads(), reads + 2);
    assert_eq!(view(&h).model(), h.state().tabs()[0].published);
    let bounds = view(&h).bodies()[&1].bounds.unwrap();
    assert_eq!(bounds[1][0], 70.0);
    assert_eq!(never_waits::waits(), 0);
}

// ---- Pointer events out of order ----

#[test]
fn a_release_with_no_press_and_a_drag_out_of_the_pane_move_the_view_only_while_held() {
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    let home = view(&h).camera().clone();
    let viewport = the_viewport(&h);
    let at = viewport.center() - vec2(100.0, 0.0);
    // Releases with no press, of every button, and a primary drag with no tool chosen.
    h.hover_at(at);
    for which in [
        PointerButton::Middle,
        PointerButton::Primary,
        PointerButton::Secondary,
    ] {
        button_event(&h, at, which, false, Modifiers::NONE);
    }
    button_event(&h, at, PointerButton::Primary, true, Modifiers::NONE);
    h.hover_at(at + vec2(50.0, 50.0));
    button_event(
        &h,
        at + vec2(50.0, 50.0),
        PointerButton::Primary,
        false,
        Modifiers::NONE,
    );
    step(&mut h);
    assert_eq!(view(&h).camera(), &home);
    // The middle button pressed in the viewport, dragged out of it over the browser, and
    // let go there: a pan by the whole way, then nothing as the pointer comes back.
    let out = egui::pos2(viewport.left() - 60.0, at.y);
    h.hover_at(at);
    button_event(&h, at, PointerButton::Middle, true, Modifiers::NONE);
    h.hover_at(out);
    button_event(&h, out, PointerButton::Middle, false, Modifiers::NONE);
    step(&mut h);
    let panned = view(&h).camera().clone();
    let mm = view_height(&home) / f64::from(viewport.height());
    let dx = f64::from(at.x - out.x);
    let frame = home.frame().unwrap();
    for k in 0..3 {
        let expected = home.target[k] + frame.right[k] * dx * mm;
        assert!((panned.target[k] - expected).abs() < 1e-9, "{panned:?}");
    }
    h.hover_at(at + vec2(30.0, 30.0));
    h.hover_at(at);
    step(&mut h);
    assert_eq!(view(&h).camera(), &panned);
    // A middle press on the ViewCube isn't the viewport's: it pans nothing.
    let cube = silico_app::viewport::view_cube_rect(viewport).center();
    h.hover_at(cube);
    button_event(&h, cube, PointerButton::Middle, true, Modifiers::NONE);
    h.hover_at(cube - vec2(80.0, 0.0));
    button_event(
        &h,
        cube - vec2(80.0, 0.0),
        PointerButton::Middle,
        false,
        Modifiers::NONE,
    );
    step(&mut h);
    assert_eq!(view(&h).camera(), &panned);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_window_too_small_for_a_viewport_draws_on_and_reads_nothing_wrong() {
    // 60 by 40 points: the panes leave the viewport a few pixels or none.
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", EXAMPLE, (60.0, 40.0));
    meshes_settled(&mut h);
    assert!(reads_over(&mut h, 100) == 0);
    let frame = record.lock().unwrap().frames.last().unwrap().clone();
    assert!(
        frame
            .camera
            .check(frame.size_px[0].max(1), frame.size_px[1].max(1))
            .is_ok()
    );
    assert_eq!(frame.bodies, [1]);
    // A wheel there: refused or a valid camera, never NaN.
    h.hover_at(the_viewport(&h).center());
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 200.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    step(&mut h);
    assert!(view(&h).camera().frame().is_ok());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn at_three_times_the_scale_past_4096_pixels_the_view_reads_once_and_settles() {
    // 7,200 by 4,800 pixels at 3× (the harness's size is the window's pixels): the viewport,
    // three fifths of the width, is over 4,096 pixels wide. Home's read is of the view's
    // shape, scaled; then nothing is read over a hundred frames.
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (7200.0, 4800.0));
    h.set_pixels_per_point(3.0);
    meshes_settled(&mut h);
    let size = view(&h).frame().unwrap().size_px;
    assert!(size[0] > MAX_RENDER_SIDE, "{size:?}");
    assert_eq!(reads_over(&mut h, 100), 0);
    // Zoomed at 2×: the same.
    h.set_pixels_per_point(2.0);
    meshes_settled(&mut h);
    assert_eq!(reads_over(&mut h, 100), 0);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_drag_whose_release_never_comes_ends_when_the_pointer_leaves_or_the_window_loses_focus() {
    // A middle press, then the pointer gone from the window (or the focus lost), whose
    // release the window may never see: the pointer coming back, moving, pans nothing,
    // and the view settles.
    for lost in [Event::PointerGone, Event::WindowFocused(false)] {
        let f = fixture();
        let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
        meshes_settled(&mut h);
        let at = the_viewport(&h).center() - vec2(100.0, 0.0);
        h.hover_at(at);
        button_event(&h, at, PointerButton::Middle, true, Modifiers::NONE);
        step(&mut h);
        h.event(lost.clone());
        step(&mut h);
        let before = view(&h).camera().clone();
        for i in 1..=5u8 {
            h.hover_at(at + vec2(10.0 * f32::from(i), 0.0));
            step(&mut h);
        }
        assert_eq!(view(&h).camera(), &before, "{lost:?}");
        assert_eq!(reads_over(&mut h, 20), 0, "{lost:?}");
        assert_eq!(never_waits::waits(), 0);
    }
}

#[test]
fn a_view_cube_drag_held_still_isnt_a_settled_view() {
    // In perspective, 900 mm in front of the large cube's centre: the small cube, 2.45 m
    // off to the left, 400 mm deep at its nearest. The cube dragged 90 points (a turn of
    // 45° about z), which puts the small cube behind the eye, nearly 2 m away: its half
    // pixel leaves its serves. Held still for twenty frames: no read until it's let go;
    // then one.
    let f = fixture();
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: [2500.0, -400.0, 500.0],
        target: [2500.0, 500.0, 500.0],
        up: [0.0, 0.0, 1.0],
    });
    meshes_settled(&mut h);
    let reads = view(&h).reads();
    let centre = silico_app::viewport::view_cube_rect(the_viewport(&h)).center();
    h.hover_at(centre);
    button_event(&h, centre, PointerButton::Primary, true, Modifiers::NONE);
    step(&mut h);
    h.hover_at(centre + vec2(90.0, 0.0));
    step(&mut h);
    assert_ne!(
        view(&h).camera().eye,
        [2500.0, -400.0, 500.0],
        "the cube turned the view"
    );
    for _ in 0..20 {
        step(&mut h);
    }
    assert_eq!(view(&h).reads(), reads);
    button_event(
        &h,
        centre + vec2(90.0, 0.0),
        PointerButton::Primary,
        false,
        Modifiers::NONE,
    );
    meshes_settled(&mut h);
    assert_eq!(view(&h).reads(), reads + 1);
    assert_eq!(never_waits::waits(), 0);
}

/// A present whose picking answers come two frames after the frame that asked, as through
/// egui_wgpu's paint callback: the GPU's answer is harvested in the next frame's prepare,
/// and taken by the pane in the frame after.
#[derive(Default)]
struct Lagging {
    presents: u64,
    waiting: std::collections::VecDeque<(u64, Picking)>,
}

struct LaggingPicks(Arc<Mutex<Lagging>>);

impl Present for LaggingPicks {
    fn present(&mut self, _: &egui::Painter, _: egui::Rect, _: Vec<Change>, f: Option<FrameData>) {
        let mut lag = self.0.lock().unwrap();
        lag.presents += 1;
        if let Some(frame) = f
            && let Some(rect) = frame.picking
        {
            let due = lag.presents + 1;
            lag.waiting.push_back((
                due,
                Picking {
                    frame: frame.serial,
                    rect,
                    hit: None,
                    edge: None,
                },
            ));
        }
    }
    fn picked(&mut self) -> Option<Picking> {
        let mut lag = self.0.lock().unwrap();
        let due = lag.waiting.front()?.0;
        (due <= lag.presents)
            .then(|| lag.waiting.pop_front().map(|(_, picking)| picking))
            .flatten()
    }
    fn refused(&mut self) -> std::collections::BTreeSet<u32> {
        std::collections::BTreeSet::new()
    }
}

#[test]
fn a_pick_answered_two_frames_late_across_a_renumber_is_taken_only_by_its_own_table() {
    // One rectangle picked in frame N, of one bodies table; the read under way answers in
    // frame N + 1, making the table again, and the same rectangle is picked again then.
    // Frame N's answer comes in N + 2: it's the old table's (the newest pick of its
    // rectangle is the new table's, but it names frame N), so it's dropped; frame N + 1's,
    // in N + 3, is taken.
    let f = fixture();
    let lag = Arc::new(Mutex::new(Lagging::default()));
    let app =
        App::new(Arc::clone(&f.sessions)).presenting(Box::new(LaggingPicks(Arc::clone(&lag))));
    let mut h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    };
    silico_app::theme::apply(&h.ctx);
    let path = f.file("a.silico", EXAMPLE);
    h.state_mut().open(&path, false);
    meshes_settled(&mut h);
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let height = f64::from(view(&h).frame().unwrap().size_px[1]);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic {
            view_height: 0.01 * height,
        },
        eye: [20.0, -500.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    });
    step_until(&mut h, "the read held", |h| view(h).reading());
    let rect_px = silico_app::renderer::PixelRect {
        min: [5, 5],
        size: [3, 3],
    };
    assert!(view_mut(&mut h).pick(rect_px));
    step(&mut h);
    // The read's answer readied before frame N + 1.
    hold.release();
    assert!(f.kernel.wait_idle(WAIT));
    std::thread::sleep(Duration::from_millis(200));
    assert!(view_mut(&mut h).pick(rect_px));
    step(&mut h);
    assert!(!view(&h).reading(), "the read's answer came in frame N + 1");
    assert_eq!(view(&h).picked(), None);
    step(&mut h);
    assert_eq!(
        view(&h).picked(),
        None,
        "frame N's answer is the old table's"
    );
    step(&mut h);
    assert_eq!(view(&h).picked().map(|picking| picking.rect), Some(rect_px));
    assert!(lag.lock().unwrap().waiting.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn highlights_of_a_body_not_drawn_or_an_element_its_mesh_hasnt_are_left_out_of_the_frame() {
    // The renderer refuses a whole frame for one: the large cube hidden, a highlight of its
    // face, and the small cube's 99th face (a box has 6), dropped; the small cube's first
    // face and twelfth edge kept.
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "the browser", |h| {
        h.query_by_label("Hide Large").is_some()
    });
    h.get_by_label("Hide Large").click();
    step(&mut h);
    use silico_app::renderer::Element;
    view_mut(&mut h).highlights = [
        (1, Element::Face(1)),
        (1, Element::Face(99)),
        (1, Element::Edge(12)),
        (1, Element::Edge(13)),
        (2, Element::Face(1)),
        (3, Element::Face(1)),
    ]
    .into_iter()
    .collect();
    step(&mut h);
    let told = record.lock().unwrap();
    let last = told.frames.last().unwrap();
    assert_eq!(last.bodies, [1]);
    assert_eq!(
        last.highlights,
        [(1, Element::Face(1)), (1, Element::Edge(12))]
    );
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn zooming_in_without_end_stops_where_the_bodies_reach_from_the_target_allows() {
    // From the front onto (25, 25, 25): the large cube's corner (3000, 1000, 1000) is
    // farthest, √(2975² + 975² + 975²) = 3,278.97 mm away, so no view under the pane's rows
    // × 3,278.97 / 2^19 · 1.01 is drawn: the wheel stops there, far above the camera's own
    // least, 1e-6 mm.
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: [25.0, -500.0, 25.0],
        target: [25.0; 3],
        up: [0.0, 0.0, 1.0],
    });
    step(&mut h);
    let reach = (2975.0f64 * 2975.0 + 975.0 * 975.0 * 2.0).sqrt();
    let least = least_in(&record, reach);
    let centre = the_viewport(&h).center();
    for _ in 0..40 {
        h.hover_at(centre);
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 200.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
    }
    // The wheel stops at the least itself (its target kept at the centre's point).
    let height = view_height(view(&h).camera());
    assert!((height - least).abs() < 1e-12, "{height} against {least}");
    // And zooms out from there: twice as high.
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -200.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    step(&mut h);
    assert!((view_height(view(&h).camera()) - 2.0 * height).abs() < 1e-12);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_zoom_in_stops_at_a_floor_as_much_higher_as_the_pane_is_taller() {
    // The same view, wheel, and bodies as above, in windows 800 and 1600 points high: the
    // pane's height in pixels sets the least, rows × 3,278.97 / 2^19 · 1.01 (the renderer's
    // f32 resolves a point to 2^-22 of its distance, at most an eighth of a pixel). The
    // taller pane has more rows, and its zoom stops higher in the same ratio.
    let mut floors = vec![];
    for tall in [800.0, 1600.0] {
        let f = fixture();
        let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, tall));
        meshes_settled(&mut h);
        view_mut(&mut h).set_camera(Camera {
            projection: Projection::Orthographic { view_height: 80.0 },
            eye: [25.0, -500.0, 25.0],
            target: [25.0; 3],
            up: [0.0, 0.0, 1.0],
        });
        step(&mut h);
        let centre = the_viewport(&h).center();
        for _ in 0..40 {
            h.hover_at(centre);
            h.event(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 200.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            });
            step(&mut h);
        }
        let reach = (2975.0f64 * 2975.0 + 975.0 * 975.0 * 2.0).sqrt();
        let height = view_height(view(&h).camera());
        let rows = rows(&record);
        let least = f64::from(rows) * reach / 524_288.0 * 1.01;
        assert!(
            (height - least).abs() < 1e-12,
            "{rows} rows: {height} against {least}"
        );
        floors.push((rows, height));
    }
    let [(low_rows, low), (high_rows, high)] = [floors[0], floors[1]];
    assert!(high_rows > low_rows && high > low, "{floors:?}");
    assert!(
        (high / low - f64::from(high_rows) / f64::from(low_rows)).abs() < 1e-12,
        "{floors:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_view_zoomed_in_before_the_first_read_answers_is_raised_to_the_least_its_bodies_allow() {
    // The first read held while the wheel zooms in fifty times at the centre: with no body
    // yet, down to the camera's own least. When the cut comes, its box (0, 0, −1) to (40,
    // 20, 11) reaches √(40² + 20² + 11²) = √2121 from the target, the origin, so the view
    // is raised to the pane's rows × √2121 / 2^19 · 1.01, and a wheel out doubles it.
    let f = fixture();
    let hold = f.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let (mut h, record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    step_until(&mut h, "the first read held", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| tab.view.reading())
    });
    let centre = the_viewport(&h).center();
    for _ in 0..50 {
        h.hover_at(centre);
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 200.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
    }
    assert!(view_height(view(&h).camera()) < 1e-5);
    assert_eq!(view(&h).camera().target, [0.0; 3]);
    hold.release();
    meshes_settled(&mut h);
    let least = least_in(&record, 2121f64.sqrt());
    let height = view_height(view(&h).camera());
    assert!((height - least).abs() < 1e-15, "{height} against {least}");
    h.hover_at(centre);
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -200.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    step(&mut h);
    assert!((view_height(view(&h).camera()) - 2.0 * least).abs() < 1e-15);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_view_at_its_least_whose_bodies_then_reach_farther_is_raised_and_moves_on() {
    // At the least the small cube allows from (25, 25, 25) (the pane's rows × its reach /
    // 2^19 · 1.01, for each reach here); then
    // another client moves the large cube a kilometre out, to (1e6, 0, 0): its far corner
    // (1,001,000, 1000, 1000) is √(1,000,975² + 975² + 975²) from the target, and the view is
    // raised to that reach's least; a pan, an orbit, and a wheel out all move it.
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    let id = h.state().tabs()[0].id;
    let revision = f.sessions.status(&id).session.unwrap().revision;
    f.sessions
        .set_field(
            &id,
            revision,
            FeatureId::new("a91cw0de").unwrap(),
            "position".to_owned(),
            json::read(b"[-2000, 0, 0]").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    meshes_settled(&mut h);
    // Out of the way while zooming: (−2000, 0, 0) to (−1000, 1000, 1000) reaches
    // √(2025² + 975² + 975²) from (25, 25, 25).
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: [25.0, -500.0, 25.0],
        target: [25.0; 3],
        up: [0.0, 0.0, 1.0],
    });
    step(&mut h);
    let centre = the_viewport(&h).center();
    for _ in 0..40 {
        h.hover_at(centre);
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 200.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
    }
    let near = least_in(&record, (2025.0f64 * 2025.0 + 2.0 * 975.0 * 975.0).sqrt());
    assert!((view_height(view(&h).camera()) - near).abs() < 1e-12);
    meshes_settled(&mut h);
    let revision = f.sessions.status(&id).session.unwrap().revision;
    f.sessions
        .set_field(
            &id,
            revision,
            FeatureId::new("a91cw0de").unwrap(),
            "position".to_owned(),
            json::read(b"[1000000, 0, 0]").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    let published = |h: &AppHarness| h.state().tabs()[0].published;
    step_until(&mut h, "the move read", |h| {
        view(h).model() == published(h) && view(h).bodies()[&2].bounds.unwrap()[0][0] == 1e6
    });
    let far = least_in(
        &record,
        (1_000_975.0f64 * 1_000_975.0 + 2.0 * 975.0 * 975.0).sqrt(),
    );
    assert!((view_height(view(&h).camera()) - far).abs() < 1e-9);
    let at = centre - vec2(100.0, 0.0);
    h.hover_at(at);
    button_event(&h, at, PointerButton::Middle, true, Modifiers::NONE);
    h.hover_at(at + vec2(30.0, 0.0));
    button_event(
        &h,
        at + vec2(30.0, 0.0),
        PointerButton::Middle,
        false,
        Modifiers::NONE,
    );
    step(&mut h);
    assert_ne!(view(&h).camera().target, [25.0; 3], "panned");
    h.hover_at(centre);
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -200.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    step(&mut h);
    assert!(view_height(view(&h).camera()) > 1.9 * far);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_view_moving_while_a_new_revision_is_refused_for_the_moment_reads_nothing_until_its_time() {
    // The bodies are of an older revision, so any view of the new one would read; its read
    // was refused for the moment. A pan of thirty frames, each another view, the time not
    // moving: no read; let go and settled, none either. At the retry's time, 250 ms on, one
    // (refused again, its next half a second off, the backoff going on).
    let f = fixture();
    let failing = failing_kernel(&f);
    let (time, clock) = hand_clock();
    let (mut h, _record) = f.drawn_clocked("boxes.silico", TWO_BOXES, clock);
    meshes_settled(&mut h);
    failing.store(true, Ordering::SeqCst);
    let id = h.state().tabs()[0].id;
    let revision = f.sessions.status(&id).session.unwrap().revision;
    let reads = view(&h).reads();
    let wakes = h.state().wakes();
    f.sessions
        .set_field(
            &id,
            revision,
            FeatureId::new("k7f2qx3m").unwrap(),
            "height".to_owned(),
            json::read(b"60").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the new revision's refused read", |h| {
        view(h).reads() > reads && !view(h).reading()
    });
    woken(&mut h, wakes);
    let refused = view(&h).reads();
    assert_eq!(refused, reads + 1);
    let at = the_viewport(&h).center() - vec2(100.0, 0.0);
    h.hover_at(at);
    button_event(&h, at, PointerButton::Middle, true, Modifiers::NONE);
    step(&mut h);
    for i in 1..=30u8 {
        h.hover_at(at + vec2(2.0 * f32::from(i), 0.0));
        step(&mut h);
    }
    assert_eq!(view(&h).reads(), refused, "a read while the view moved");
    button_event(
        &h,
        at + vec2(60.0, 0.0),
        PointerButton::Middle,
        false,
        Modifiers::NONE,
    );
    assert_eq!(reads_over(&mut h, 20), 0);
    advance(&time, Duration::from_millis(250));
    assert_eq!(reads_over(&mut h, 5), 1);
    // Refused again: nothing more until half a second has passed.
    assert_eq!(reads_over(&mut h, 20), 0);
    advance(&time, Duration::from_millis(500));
    assert_eq!(reads_over(&mut h, 5), 1);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn wheel_notches_during_a_backoff_read_at_most_once() {
    // A new revision refused for the moment; five wheel notches, each followed by a frame
    // the view rests in (settled): no read before the retry's time, 250 ms on, and one at
    // it, of the view then.
    let f = fixture();
    let failing = failing_kernel(&f);
    let (time, clock) = hand_clock();
    let (mut h, _record) = f.drawn_clocked("boxes.silico", TWO_BOXES, clock);
    meshes_settled(&mut h);
    failing.store(true, Ordering::SeqCst);
    let id = h.state().tabs()[0].id;
    let revision = f.sessions.status(&id).session.unwrap().revision;
    let reads = view(&h).reads();
    let wakes = h.state().wakes();
    f.sessions
        .set_field(
            &id,
            revision,
            FeatureId::new("k7f2qx3m").unwrap(),
            "height".to_owned(),
            json::read(b"60").unwrap(),
        )
        .wait()
        .result
        .unwrap();
    step_until(&mut h, "the new revision's refused read", |h| {
        view(h).reads() > reads && !view(h).reading()
    });
    woken(&mut h, wakes);
    let refused = view(&h).reads();
    let centre = the_viewport(&h).center();
    for _ in 0..5 {
        h.hover_at(centre);
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 40.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
        step(&mut h);
        step(&mut h);
    }
    assert_eq!(reads_over(&mut h, 10), 0);
    assert_eq!(view(&h).reads(), refused);
    let rested = view(&h).camera().clone();
    advance(&time, Duration::from_millis(250));
    assert_eq!(reads_over(&mut h, 10), 1);
    assert_eq!(view(&h).camera(), &rested);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_body_shown_again_raises_a_view_under_the_least_its_reach_allows() {
    // The large cube hidden, the view zoomed in to the least the small one allows from
    // (25, 25, 25); the large cube shown again: in the next frame the view is raised to
    // the least of both, so the renderer draws it. The viewport's time is the test's, so no
    // retry falls due on a slow runner.
    let f = fixture();
    let (_time, clock) = hand_clock();
    let (mut h, record) = f.drawn_clocked("boxes.silico", TWO_BOXES, clock);
    meshes_settled(&mut h);
    step_until(&mut h, "the browser", |h| {
        h.query_by_label("Hide Large").is_some()
    });
    h.get_by_label("Hide Large").click();
    step(&mut h);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: [25.0, -500.0, 25.0],
        target: [25.0; 3],
        up: [0.0, 0.0, 1.0],
    });
    step(&mut h);
    let centre = the_viewport(&h).center();
    for _ in 0..40 {
        h.hover_at(centre);
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 200.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        step(&mut h);
    }
    let small = least_in(&record, (3.0f64 * 25.0 * 25.0).sqrt());
    assert!((view_height(view(&h).camera()) - small).abs() < 1e-15);
    let boxes: Vec<[[f64; 3]; 2]> = view(&h)
        .bodies()
        .values()
        .filter_map(|body| body.bounds)
        .collect();
    assert!(!navigate::drawable(
        view(&h).camera(),
        rows(&record),
        boxes.clone()
    ));
    // No read lands across the show (its answer would clamp the view too): the kernel
    // refuses to mesh, and the read count is the same after.
    let failing = failing_kernel(&f);
    failing.store(true, Ordering::SeqCst);
    step_until(&mut h, "no read out", |h| !view(h).reading());
    let reads = view(&h).reads();
    h.get_by_label("Show Large").click();
    step(&mut h);
    step(&mut h);
    assert_eq!(view(&h).reads(), reads);
    assert!(navigate::drawable(
        view(&h).camera(),
        rows(&record),
        boxes.clone()
    ));
    let both = least_in(&record, (2975.0f64 * 2975.0 + 2.0 * 975.0 * 975.0).sqrt());
    assert!((view_height(view(&h).camera()) - both).abs() < 1e-12);
    let last = record.lock().unwrap().frames.last().unwrap().clone();
    assert_eq!(last.bodies, [1, 2]);
    assert!(navigate::drawable(&last.camera, last.size_px[1], boxes));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_retry_falling_due_mid_pan_keeps_its_run_growing() {
    // Refused once (the next in 250 ms); the time moved past it while the middle button
    // pans, frame after frame: nothing is read, and the run isn't ended. Let go: the view
    // at rest is read, refused again, and the next waits 500 ms, not 250.
    let f = fixture();
    let failing = failing_kernel(&f);
    let (time, clock) = hand_clock();
    let (mut h, _record) = f.drawn_clocked("boxes.silico", TWO_BOXES, clock);
    meshes_settled(&mut h);
    failing.store(true, Ordering::SeqCst);
    let reads = view(&h).reads();
    let wakes = h.state().wakes();
    let zoomed = the_small_cube_at_0_04(&h);
    view_mut(&mut h).set_camera(zoomed);
    step_until(&mut h, "the refused read", |h| {
        view(h).reads() > reads && !view(h).reading()
    });
    woken(&mut h, wakes);
    let at = the_viewport(&h).center() - vec2(100.0, 0.0);
    h.hover_at(at);
    button_event(&h, at, PointerButton::Middle, true, Modifiers::NONE);
    step(&mut h);
    advance(&time, Duration::from_millis(300));
    for i in 1..=10u8 {
        h.hover_at(at + vec2(2.0 * f32::from(i), 0.0));
        step(&mut h);
    }
    assert_eq!(view(&h).reads(), reads + 1);
    let wakes = h.state().wakes();
    button_event(
        &h,
        at + vec2(20.0, 0.0),
        PointerButton::Middle,
        false,
        Modifiers::NONE,
    );
    step_until(&mut h, "the read at rest refused", |h| {
        view(h).reads() == reads + 2 && !view(h).reading()
    });
    woken(&mut h, wakes);
    for _ in 0..3 {
        step(&mut h);
    }
    let delay = repaint_delay(&h).unwrap_or_default();
    assert!(
        delay <= Duration::from_millis(500) && delay > Duration::from_millis(480),
        "{delay:?}"
    );
    assert_eq!(never_waits::waits(), 0);
}

/// A [`Recorder`] whose renderer says it backs off while `0` is set.
struct BackingOff(Recorder, Arc<AtomicBool>);

impl Present for BackingOff {
    fn present(
        &mut self,
        painter: &egui::Painter,
        rect: egui::Rect,
        changes: Vec<Change>,
        frame: Option<FrameData>,
    ) {
        self.0.present(painter, rect, changes, frame);
    }
    fn picked(&mut self) -> Option<Picking> {
        self.0.picked()
    }
    fn refused(&mut self) -> std::collections::BTreeSet<u32> {
        self.0.refused()
    }
    fn backing_off(&mut self) -> bool {
        self.1.load(Ordering::SeqCst)
    }
}

#[test]
fn a_renderer_backing_off_has_the_pane_ask_for_each_frame_at_once_until_it_stops() {
    // A renderer counts its backoff in frames (silico-render's, 1 to 64 of them after the
    // GPU fails one): an idle pane, asking a frame each idle interval, would show nothing
    // for 64 of those. While it backs off, each frame asks for the next at once; once it
    // doesn't, the frames are the idle ones again.
    let f = fixture();
    let record = Record::default();
    let backing_off = Arc::new(AtomicBool::new(false));
    let present = BackingOff(
        Recorder(FakeRenderer(Arc::clone(&record))),
        Arc::clone(&backing_off),
    );
    let app = App::new(Arc::clone(&f.sessions)).presenting(Box::new(present));
    let mut h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    };
    silico_app::theme::apply(&h.ctx);
    let path = f.file("boxes.silico", TWO_BOXES);
    h.state_mut().open(&path, false);
    meshes_settled(&mut h);
    // An idle frame: one asked for at most IDLE_FRAME on (less the time the frames took),
    // never at once.
    let idle = |delay: Option<Duration>| {
        delay.is_some_and(|delay| {
            delay > Duration::from_millis(100) && delay <= silico_app::app::IDLE_FRAME
        })
    };
    // Settled: egui repaints once more after a frame asked at once, so a few frames.
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(idle(repaint_delay(&h)), "{:?}", repaint_delay(&h));
    backing_off.store(true, Ordering::SeqCst);
    for _ in 0..10 {
        step(&mut h);
        assert_eq!(repaint_delay(&h), Some(Duration::ZERO));
    }
    backing_off.store(false, Ordering::SeqCst);
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(idle(repaint_delay(&h)), "{:?}", repaint_delay(&h));
    assert_eq!(never_waits::waits(), 0);
}

/// A document of one sketch on XY, Huge, of `circles` circles of radius 100 m about the
/// origin, nothing built from it.
fn huge_sketch(circles: usize) -> String {
    let members: Vec<String> = (0..circles)
        .map(|k| {
            format!(
                r#"{{"id": "c{k:07}", "center": "pa000001", "radius": 100000, "construction": false}}"#
            )
        })
        .collect();
    format!(
        r#"{{
  "silico": 1,
  "units": {{"length": "mm", "resolution": "1nm"}},
  "naming": 5,
  "features": [
    {{"id": "s0000001", "type": "sketch", "v": 1, "name": "Huge", "plane": "xy", "points": [{{"id": "pa000001", "x": 0, "y": 0, "construction": false}}], "lines": [], "circles": [{}], "arcs": [], "constraints": []}}
  ]
}}
"#,
        members.join(", ")
    )
}

/// A document of `sketches` sketches on XY, each of `circles` circles of radius 100 m about
/// the origin, nothing built from them.
fn huge_sketches(sketches: usize, circles: usize) -> String {
    let sketch = |k: usize| {
        let members: Vec<String> = (0..circles)
            .map(|c| {
                format!(
                    r#"{{"id": "c{c:07}", "center": "pa000001", "radius": 100000, "construction": false}}"#
                )
            })
            .collect();
        format!(
            r#"    {{"id": "s{k:07}", "type": "sketch", "v": 1, "name": "Huge{k}", "plane": "xy", "points": [{{"id": "pa000001", "x": 0, "y": 0, "construction": false}}], "lines": [], "circles": [{}], "arcs": [], "constraints": []}}"#,
            members.join(", ")
        )
    };
    let features: Vec<String> = (1..=sketches).map(sketch).collect();
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n{}\n  ]\n}}\n",
        features.join(",\n")
    )
}

#[test]
fn lines_left_to_build_ask_for_the_next_frame_at_once_and_none_once_built() {
    // Eight sketches of 195 circles of 100 m about the origin, seen from above 400 mm high,
    // then 200: each circle at its most segments, 1,024 (1,025 points), so each sketch
    // 199,875 points, two built a frame (500,000). At the new bucket the frames build two
    // each, the rest left with their old lines drawn: each frame that leaves some asks for
    // the next at once (past the first, which the view's move asks for itself); once all
    // are built, the frames are idle again.
    let f = fixture();
    let (mut h, record) = f.drawn("huge.silico", &huge_sketches(8, 195), (1280.0, 800.0));
    let ids: Vec<FeatureId> = (1..=8)
        .map(|k| FeatureId::new(&format!("s{k:07}")).unwrap())
        .collect();
    step_until(&mut h, "the eight read", |h| {
        h.state()
            .tabs()
            .first()
            .is_some_and(|tab| ids.iter().all(|id| tab.sketch_reads.held(id).is_some()))
    });
    let top = |view_height: f64| Camera {
        projection: Projection::Orthographic { view_height },
        eye: [0.0, 0.0, 1000.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
    };
    let pending = |h: &AppHarness| h.state().tabs()[0].sketch_lines.pending();
    view_mut(&mut h).set_camera(top(400.0));
    step_until(&mut h, "the eight drawn", |h| {
        !pending(h)
            && record
                .lock()
                .unwrap()
                .frames
                .last()
                .is_some_and(|frame| frame.lines.len() == 8)
    });
    let idle = |delay: Option<Duration>| {
        delay.is_some_and(|delay| {
            delay > Duration::from_millis(100) && delay <= silico_app::app::IDLE_FRAME
        })
    };
    // Settled: egui repaints once more after a frame asked at once, so a few frames.
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(idle(repaint_delay(&h)), "{:?}", repaint_delay(&h));
    view_mut(&mut h).set_camera(top(200.0));
    step(&mut h);
    assert!(pending(&h));
    let mut frames = 1;
    while pending(&h) {
        step(&mut h);
        frames += 1;
        if pending(&h) {
            assert_eq!(repaint_delay(&h), Some(Duration::ZERO), "frame {frames}");
        }
        assert!(frames <= 4, "pending after {frames} frames");
    }
    assert_eq!(frames, 4);
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(idle(repaint_delay(&h)), "{:?}", repaint_delay(&h));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_shown_sketch_past_its_lines_cap_draws_none_and_the_frames_go_on() {
    // 200 circles of 100 m, each at its most segments at the view the document opens in
    // (Home's, a tenth of a metre across: 1024 segments, 1,025 points), 205,000 points in
    // all, past the cap's 200,000. Its read held, it's drawn by no line, and the frames go
    // on, none waiting.
    let f = fixture();
    let (mut h, record) = f.drawn("huge.silico", &huge_sketch(200), (1280.0, 800.0));
    step_until(&mut h, "Huge read", |h| {
        h.state().tabs().first().is_some_and(|tab| {
            tab.sketch_reads
                .held(&FeatureId::new("s0000001").unwrap())
                .is_some()
        })
    });
    let frames = h.state().frames();
    for _ in 0..20 {
        step(&mut h);
        let told = record.lock().unwrap();
        assert!(
            told.frames
                .last()
                .is_some_and(|frame| frame.lines.is_empty())
        );
    }
    assert!(h.state().frames() >= frames + 20);
    assert_eq!(never_waits::waits(), 0);
}

// ---- The navigation bar's and the ViewCube's buttons (the prototype audit's item 11), clicked
// while the rebuild is held, in windows too small for them, many times over, and with a menu
// open as its tab closes ----

/// Every button of the navigation bar and the ViewCube's own (not its faces, edges, and
/// corners): its holder's role and label, and its own label.
const BAR_AND_CUBE_BUTTONS: [(egui::accesskit::Role, &str, &str); 9] = {
    use egui::accesskit::Role::{Group, Toolbar};
    [
        (Toolbar, "Navigation bar", "Orbit"),
        (Toolbar, "Navigation bar", "Pan"),
        (Toolbar, "Navigation bar", "Zoom"),
        (Toolbar, "Navigation bar", "Fit"),
        (Toolbar, "Navigation bar", "Look At"),
        (Toolbar, "Navigation bar", "Display settings"),
        (Toolbar, "Navigation bar", "Grid"),
        (Group, "ViewCube", "Home view"),
        (Group, "ViewCube", "View cube options"),
    ]
};

/// The button `label` of the holder `holder` (`role`).
fn button<'a>(
    h: &'a AppHarness,
    role: egui::accesskit::Role,
    holder: &'a str,
    label: &'a str,
) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(role, holder)
        .get_by_role_and_label(egui::accesskit::Role::Button, label)
}

/// Whether any menu is open over the window.
fn a_menu_open(h: &AppHarness) -> bool {
    h.query_all_by_role(egui::accesskit::Role::Menu)
        .next()
        .is_some()
}

#[test]
fn each_bar_and_cube_button_clicked_while_the_rebuild_is_held_waits_on_nothing() {
    // Every kernel request held from the open: the first rebuild runs and publishes nothing.
    // Each of the bar's and the cube's buttons clicked meanwhile, a frame apart: Orbit, Pan,
    // and Zoom chosen in turn; Fit with no bodies fits nothing; Look At with nothing selected
    // says so; the menus open, and close at the next click. Never a wait on the UI thread,
    // and the camera always one a frame draws. Released, the model is published and read.
    let f = fixture();
    let hold = f.kernel.hold(|_| true);
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(h.state().tabs()[0].published.is_none());
    for (role, holder, label) in BAR_AND_CUBE_BUTTONS {
        button(&h, role, holder, label).click();
        step(&mut h);
        step(&mut h);
        let camera = view(&h).camera();
        assert!(camera.frame().is_ok(), "{label}: {camera:?}");
        if label == "Look At" {
            let said = h.state().toast().map(|(text, _)| text.to_owned());
            assert_eq!(said.as_deref(), Some(silico_app::tab::LOOK_AT_NO_FACE));
        }
    }
    assert_eq!(view(&h).tool, Some(Navigation::Zoom));
    assert!(!view(&h).grid);
    hold.release();
    meshes_settled(&mut h);
    assert!(h.state().tabs()[0].published.is_some());
    assert!(!view(&h).bodies().is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn in_windows_too_small_for_the_bar_its_buttons_keep_finite_bounds_and_still_answer() {
    // 60 by 40 points, and 400 by 300: the canvas a few points or none, or narrower than
    // the bar's 220. The bar and the cube still in the tree, every bound finite; each
    // button clicked as a screen reader clicks it (its action, where the pointer can't
    // reach): Grid hides the grid, Orbit is chosen, and the camera is always one a frame
    // draws; nothing waits.
    for size in [(60.0, 40.0), (400.0, 300.0)] {
        let f = fixture();
        let (mut h, _record) = f.drawn("a.silico", EXAMPLE, size);
        meshes_settled(&mut h);
        for (role, holder, label) in BAR_AND_CUBE_BUTTONS {
            let holder_rect = h.get_by_role_and_label(role, holder).rect();
            let rect = button(&h, role, holder, label).rect();
            for k in [
                holder_rect.min.x,
                holder_rect.min.y,
                holder_rect.max.x,
                holder_rect.max.y,
                rect.min.x,
                rect.min.y,
                rect.max.x,
                rect.max.y,
            ] {
                assert!(
                    k.is_finite(),
                    "{size:?} {label}: {rect:?} in {holder_rect:?}"
                );
            }
        }
        for label in ["Grid", "Orbit"] {
            button(&h, egui::accesskit::Role::Toolbar, "Navigation bar", label).click_accesskit();
            step(&mut h);
            step(&mut h);
        }
        assert!(!view(&h).grid, "{size:?}");
        assert_eq!(view(&h).tool, Some(Navigation::Orbit), "{size:?}");
        for (role, holder, label) in BAR_AND_CUBE_BUTTONS {
            button(&h, role, holder, label).click_accesskit();
            step(&mut h);
            step(&mut h);
            let camera = view(&h).camera();
            assert!(camera.frame().is_ok(), "{size:?} {label}: {camera:?}");
        }
        assert_eq!(never_waits::waits(), 0);
    }
}

#[test]
fn the_bars_and_the_cubes_toggles_clicked_nine_times_a_frame_apart_end_as_one_click_leaves_them() {
    // Each clicked nine times, a frame apart (within egui's double-click time, so the clicks
    // pair up as double-clicks too): each click one toggle, so each ends as one click leaves
    // it. Grid: hidden. Orbit: chosen. The ▾: its menu open. Display settings: its menu
    // open, and the cube's closed (one popup at a time).
    use egui::accesskit::Role::{Group, Menu, Toolbar};
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    let nine = |h: &mut AppHarness, role, holder: &str, label: &str| {
        for _ in 0..9 {
            button(h, role, holder, label).click();
            step(h);
        }
        step(h);
        step(h);
    };
    nine(&mut h, Toolbar, "Navigation bar", "Grid");
    assert!(!view(&h).grid);
    nine(&mut h, Toolbar, "Navigation bar", "Orbit");
    assert_eq!(view(&h).tool, Some(Navigation::Orbit));
    nine(&mut h, Group, "ViewCube", "View cube options");
    assert!(
        h.query_by_role_and_label(Menu, "View cube options")
            .is_some()
    );
    nine(&mut h, Toolbar, "Navigation bar", "Display settings");
    assert!(
        h.query_by_role_and_label(Menu, "Display settings")
            .is_some()
    );
    assert!(
        h.query_by_role_and_label(Menu, "View cube options")
            .is_none()
    );
    // The camera through it all one a frame draws, and Orbit still chosen.
    assert!(view(&h).camera().frame().is_ok());
    assert_eq!(view(&h).tool, Some(Navigation::Orbit));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_menu_open_over_the_canvas_as_its_tab_closes_goes_with_it() {
    // The display settings' menu open, then the tab closed (Ctrl+W; its file saved, so
    // nothing is asked): the menu goes with the bar, nothing waits, and the cube's menu
    // the same way. A document opened after shows neither open.
    use egui::accesskit::Role::{Group, Toolbar};
    for (role, holder, label) in [
        (Toolbar, "Navigation bar", "Display settings"),
        (Group, "ViewCube", "View cube options"),
    ] {
        let f = fixture();
        let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
        meshes_settled(&mut h);
        button(&h, role, holder, label).click();
        step(&mut h);
        step(&mut h);
        assert!(a_menu_open(&h), "{label}");
        h.key_press_modifiers(Modifiers::COMMAND, egui::Key::W);
        step_until(&mut h, "the tab closed", |h| h.state().tabs().is_empty());
        step(&mut h);
        step(&mut h);
        assert!(!a_menu_open(&h), "{label}");
        let path = f.file("b.silico", EXAMPLE);
        h.state_mut().open(&path, false);
        meshes_settled(&mut h);
        assert!(!a_menu_open(&h), "{label}");
        assert_eq!(never_waits::waits(), 0);
    }
}

// ---- The triad and the view label at the canvas's bottom left ----

#[test]
fn a_window_too_small_for_a_viewport_clips_the_triad_and_its_label_to_what_is_left() {
    // 60 by 40 points, the viewport a few points or none: the triad's origin, 44 in and up
    // from the canvas's corner, is off it, and the label's line, 82 in, too. Each is still
    // laid out, finite, and painted only within the viewport, as the prototype's
    // `overflow: hidden` canvas clips its overlay; the label says the home view.
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (60.0, 40.0));
    meshes_settled(&mut h);
    step(&mut h);
    let viewport = the_viewport(&h);
    assert_eq!(view_label(&h), "Orthographic · Iso");
    let origin = triad_origin(viewport);
    let mut seen = 0;
    for clipped in &h.output().shapes {
        let of_the_triad = match &clipped.shape {
            egui::Shape::Circle(c) => c.center == origin && c.fill == TRIAD_DOT,
            egui::Shape::LineSegment { points, stroke } => {
                points[0] == origin && [AXIS_X, AXIS_Y, AXIS_Z].contains(&stroke.color)
            }
            egui::Shape::Text(t) => {
                t.galley.text() == "Orthographic · Iso"
                    || (["X", "Y", "Z"].contains(&t.galley.text())
                        && [AXIS_X, AXIS_Y, AXIS_Z].contains(&t.fallback_color))
            }
            _ => false,
        };
        if of_the_triad {
            seen += 1;
            let clip = clipped.clip_rect;
            assert!(
                !clip.is_positive() || viewport.contains_rect(clip),
                "{clip:?} past {viewport:?}: {:?}",
                clipped.shape
            );
            let at = clipped.shape.visual_bounding_rect();
            assert!(!at.is_positive() || at.is_finite(), "{:?}", clipped.shape);
        }
    }
    // Three strokes, three letters, the dot, and the label.
    assert_eq!(seen, 8);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_view_changed_every_frame_is_named_by_the_label_of_the_frame_that_draws_it() {
    // A standard view's key every frame, three times round, as fast as frames come: each
    // key is taken before the canvas is drawn, so the frame the renderer is given looks
    // from the key's view, and the label of that same frame names it. Two keys in one
    // frame: the second's view, drawn and named. A ViewCube click is taken after the canvas
    // is drawn: for a frame the old view is drawn and named, the next frame both the new.
    use egui::Key::{Home, Num1, Num2, Num3, Num4, Num5, Num6, Num7};
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    let d = 1.0 / 3f64.sqrt();
    let ctrl = Modifiers::COMMAND;
    let keys = [
        (ctrl, Num1, "Front", [0.0, -1.0, 0.0]),
        (ctrl, Num2, "Back", [0.0, 1.0, 0.0]),
        (ctrl, Num3, "Left", [-1.0, 0.0, 0.0]),
        (ctrl, Num4, "Right", [1.0, 0.0, 0.0]),
        (ctrl, Num5, "Top", [0.0, 0.0, 1.0]),
        (ctrl, Num6, "Bottom", [0.0, 0.0, -1.0]),
        (ctrl, Num7, "Iso", [d, -d, d]),
        (Modifiers::NONE, Home, "Iso", [d, -d, d]),
    ];
    let drawn = |record: &Record| record.lock().unwrap().frames.last().unwrap().camera.clone();
    for _ in 0..3 {
        for (held, key, name, way) in keys {
            h.key_press_modifiers(held, key);
            step(&mut h);
            let camera = drawn(&record);
            assert!(looks_from(&camera, way), "{name}: {camera:?}");
            assert_eq!(view_label(&h), format!("Orthographic · {name}"));
        }
    }
    h.key_press_modifiers(ctrl, Num1);
    h.key_press_modifiers(ctrl, Num5);
    step(&mut h);
    assert!(looks_from(&drawn(&record), [0.0, 0.0, 1.0]));
    assert_eq!(view_label(&h), "Orthographic · Top");
    // The cube's top front edge, clickable from the top: its view, between the top's and
    // the front's, is unnamed.
    h.get_by_role_and_label(egui::accesskit::Role::Group, "ViewCube")
        .get_by_label("Top front")
        .click();
    step(&mut h);
    assert!(looks_from(&drawn(&record), [0.0, 0.0, 1.0]));
    assert_eq!(view_label(&h), "Orthographic · Top");
    step(&mut h);
    let r = std::f64::consts::FRAC_1_SQRT_2;
    assert!(looks_from(&drawn(&record), [0.0, -r, r]));
    assert_eq!(view_label(&h), "Orthographic");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn an_orbit_a_point_a_frame_keeps_the_name_for_half_a_degree_and_drops_it_at_one() {
    // The navigation bar's Orbit, its left drag moved one point a frame from the front
    // view: half a degree a point (0.008727 rad) is inside the prototype's hundredth of a
    // radian, so the first frame's view is still Front; the second's, 1° (0.017453), isn't.
    use egui::accesskit::Role::Toolbar;
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Num1);
    step(&mut h);
    assert_eq!(view_label(&h), "Orthographic · Front");
    h.get_by_role_and_label(Toolbar, "Navigation bar")
        .get_by_label("Orbit")
        .click();
    step(&mut h);
    step(&mut h);
    let at = the_viewport(&h).center();
    h.hover_at(at);
    step(&mut h);
    button_event(&h, at, PointerButton::Primary, true, Modifiers::NONE);
    step(&mut h);
    assert_eq!(view_label(&h), "Orthographic · Front");
    h.hover_at(at + vec2(1.0, 0.0));
    step(&mut h);
    assert_eq!(view_label(&h), "Orthographic · Front");
    h.hover_at(at + vec2(2.0, 0.0));
    step(&mut h);
    assert_eq!(view_label(&h), "Orthographic");
    // Back by as much: the front again, to within its rounding.
    h.hover_at(at);
    step(&mut h);
    assert_eq!(view_label(&h), "Orthographic · Front");
    button_event(&h, at, PointerButton::Primary, false, Modifiers::NONE);
    step(&mut h);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn from_straight_above_the_z_axis_is_its_letter_alone_at_the_dot_and_drawn_last() {
    // Ctrl+5, the top view: Z along the view, toward the viewer, so no stroke of it is
    // painted, its letter stands at the origin over X's and Y's strokes, and the dot over it.
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Num5);
    step(&mut h);
    step(&mut h);
    let viewport = the_viewport(&h);
    let origin = triad_origin(viewport);
    let shapes: Vec<egui::Shape> = h
        .output()
        .shapes
        .iter()
        .map(|clipped| clipped.shape.clone())
        .collect();
    let strokes: Vec<(usize, egui::Color32)> = shapes
        .iter()
        .enumerate()
        .filter_map(|(k, shape)| match shape {
            egui::Shape::LineSegment { points, stroke }
                if points[0] == origin && [AXIS_X, AXIS_Y, AXIS_Z].contains(&stroke.color) =>
            {
                Some((k, stroke.color))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        strokes.iter().map(|(_, c)| *c).collect::<Vec<_>>(),
        [AXIS_X, AXIS_Y]
    );
    let z = shapes
        .iter()
        .position(|shape| {
            matches!(shape, egui::Shape::Text(t) if t.galley.text() == "Z" && t.fallback_color == AXIS_Z)
        })
        .unwrap();
    let dot = shapes
        .iter()
        .position(|shape| matches!(shape, egui::Shape::Circle(c) if c.center == origin && c.fill == TRIAD_DOT))
        .unwrap();
    assert!(
        strokes.iter().all(|(k, _)| *k < z) && z < dot,
        "{strokes:?} {z} {dot}"
    );
    let egui::Shape::Text(letter) = &shapes[z] else {
        unreachable!()
    };
    let middle = letter.pos.x + letter.galley.size().x / 2.0;
    assert!((middle - origin.x).abs() < 1e-3, "{letter:?}");
    assert_eq!(view_label(&h), "Orthographic · Top");
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn each_tab_shows_its_own_views_label_and_only_the_shown_tabs() {
    // Two documents, a at the front view and b at the top: the label is the shown tab's,
    // one label only, and follows each switch the frame it's shown.
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Num1);
    step(&mut h);
    let path = f.file("b.silico", TWO_BOXES);
    h.state_mut().open(&path, false);
    step_until(&mut h, "b shown", |h| h.state().tabs().len() == 2);
    meshes_settled_in(&mut h, 1);
    assert_eq!(view_label(&h), "Orthographic · Iso");
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Num5);
    step(&mut h);
    assert_eq!(view_label(&h), "Orthographic · Top");
    for (name, said) in [
        ("a.silico", "Orthographic · Front"),
        ("b.silico", "Orthographic · Top"),
        ("a.silico", "Orthographic · Front"),
    ] {
        h.get_by_label(name).click();
        step(&mut h);
        step(&mut h);
        assert_eq!(view_label(&h), said, "{name} shown");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_camera_that_makes_no_frame_draws_no_triad_and_the_label_says_the_projection_alone() {
    // A camera with no frame (its eye at its target, or a NaN in its up), set as a control
    // would set it, kept as it is: no triad is painted, the label says only
    // "Orthographic", and the next view the person asks for draws and names again.
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h);
    for broken in [
        Camera {
            eye: [0.0; 3],
            ..front(80.0)
        },
        Camera {
            up: [0.0, f64::NAN, 1.0],
            ..front(80.0)
        },
    ] {
        view_mut(&mut h).set_camera(broken.clone());
        step(&mut h);
        step(&mut h);
        let viewport = the_viewport(&h);
        let origin = triad_origin(viewport);
        let dots = h
            .output()
            .shapes
            .iter()
            .filter(|clipped| {
                matches!(&clipped.shape, egui::Shape::Circle(c) if c.center == origin && c.fill == TRIAD_DOT)
            })
            .count();
        assert!(
            view(&h).camera().frame().is_err(),
            "{:?}",
            view(&h).camera()
        );
        assert_eq!(dots, 0, "{broken:?}");
        assert_eq!(view_label(&h), "Orthographic", "{broken:?}");
        h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Num1);
        step(&mut h);
        assert_eq!(view_label(&h), "Orthographic · Front");
        assert!(view(&h).camera().frame().is_ok());
    }
    assert_eq!(never_waits::waits(), 0);
}

// ---- Hover linking: a feature hovered in the timeline tints its faces, a face hovered in
// the canvas rings its feature's item ----

/// TWO_BOXES with the large cube taken out: the small one alone, body 1.
const SMALL_ALONE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Small", "position": [0, 0, 0], "length": 50, "width": 50, "height": 50}
  ]
}
"#;

/// TWO_BOXES with the small cube taken out: the large one alone, body 1 now.
const LARGE_ALONE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "a91cw0de", "type": "box", "v": 1, "name": "Large", "position": [2000, 0, 0], "length": 1000, "width": 1000, "height": 1000}
  ]
}
"#;

/// The timeline's item `name`, if it shows.
fn item<'a>(h: &'a AppHarness, name: &'a str) -> Option<egui_kittest::Node<'a>> {
    h.get_by_role_and_label(egui::accesskit::Role::Pane, "Timeline")
        .query_by_label(name)
}

/// Body `body`'s faces 1 to 6, a box's.
fn box_faces(body: u32) -> Vec<(u32, silico_app::renderer::Element)> {
    (1..=6)
        .map(|n| (body, silico_app::renderer::Element::Face(n)))
        .collect()
}

/// The last frame's tinted faces.
fn tinted(record: &Record) -> Vec<(u32, silico_app::renderer::Element)> {
    record.lock().unwrap().frames.last().unwrap().tinted.clone()
}

/// Every pick from now on finds `hit`, a body and its element, at its square's centre.
fn finding(record: &Record, hit: Option<(u32, silico_app::renderer::Element)>) {
    record.lock().unwrap().hit = hit.map(|(body, hit)| silico_app::renderer::Pick {
        body,
        hit,
        pixel: [0, 0],
        point: None,
    });
}

/// The feature whose item tab `at`'s timeline rings, as it last drew.
fn ringed(h: &AppHarness, at: usize) -> Option<String> {
    h.state().tabs()[at]
        .view
        .ringed_feature
        .as_ref()
        .map(|id| id.as_str().to_owned())
}

#[test]
fn a_feature_removed_on_disk_while_hovered_is_tinted_in_no_frame_after_and_none_is_refused() {
    // The large cube hovered in the timeline, then taken out of the file by another program
    // (as an agent's commit is): the session reloads it, the pointer still where the item
    // was. Body 2 is the large cube's while both are drawn; once the small one is drawn
    // alone, as body 1, nothing may be tinted (its faces aren't the large cube's), and no
    // frame lists a body it doesn't draw (the renderer would refuse it whole).
    let f = watching();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    assert_eq!((body_of(&h, "k7f2qx3m"), body_of(&h, "a91cw0de")), (1, 2));
    hover_item(&mut h, "Large");
    assert_eq!(tinted(&record), box_faces(2));
    let from = record.lock().unwrap().frames.len();
    f.file("boxes.silico", SMALL_ALONE);
    step_until(&mut h, "the large cube gone", |h| {
        item(h, "Large").is_none() && view(h).bodies().len() == 1
    });
    meshes_settled(&mut h);
    for _ in 0..3 {
        step(&mut h);
    }
    let told = record.lock().unwrap();
    assert!(told.refused.is_empty(), "{:?}", told.refused);
    for frame in &told.frames[from..] {
        match frame.bodies.as_slice() {
            [1, 2] => assert!(
                frame.tinted.is_empty() || frame.tinted == box_faces(2),
                "{:?}",
                frame.tinted
            ),
            _ => assert!(frame.tinted.is_empty(), "{:?}", frame.tinted),
        }
    }
    assert_eq!(told.frames.last().unwrap().bodies, [1]);
    assert!(told.frames.last().unwrap().tinted.is_empty());
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_feature_renumbered_by_a_reload_mid_hover_is_tinted_under_its_new_number_alone() {
    // The small cube taken out under the large one's hover: the large one is body 1 now. A
    // frame drawing both tints body 2's faces or none; one drawing it alone, body 1's or
    // none, never body 2's (not drawn: refused) nor, under the old table, body 1's (the small
    // cube's). Hovered again where its item is now: its six faces, as body 1.
    let f = watching();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    hover_item(&mut h, "Large");
    assert_eq!(tinted(&record), box_faces(2));
    let from = record.lock().unwrap().frames.len();
    f.file("boxes.silico", LARGE_ALONE);
    step_until(&mut h, "the small cube gone", |h| {
        item(h, "Small").is_none() && view(h).bodies().len() == 1
    });
    meshes_settled(&mut h);
    assert_eq!(body_of(&h, "a91cw0de"), 1);
    hover_item(&mut h, "Large");
    let told = record.lock().unwrap();
    assert!(told.refused.is_empty(), "{:?}", told.refused);
    for frame in &told.frames[from..] {
        let want = match frame.bodies.as_slice() {
            [1, 2] => box_faces(2),
            [1] => box_faces(1),
            other => panic!("bodies {other:?}"),
        };
        assert!(
            frame.tinted.is_empty() || frame.tinted == want,
            "{:?} of {:?}",
            frame.tinted,
            frame.bodies
        );
    }
    assert_eq!(told.frames.last().unwrap().tinted, box_faces(1));
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_hover_left_in_a_tab_tints_nothing_in_another_or_in_it_when_shown_again() {
    // a's large cube hovered, then b opened over it with the pointer left on the timeline:
    // a is hidden with its feature hovered. The item under the pointer in b is its pin,
    // a91cw0de, the large cube's ID in a: no face of b is the pin's (its cut keeps the hole's
    // walls as its own), so nothing is tinted. b's block hovered: the faces b's block made,
    // in b's body. a shown again by its tab, the pointer on the tab: no frame of it tints
    // anything, its hover of before let go.
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    hover_item(&mut h, "Large");
    assert_eq!(tinted(&record), box_faces(2));
    let path = f.file("b.silico", EXAMPLE);
    h.state_mut().open(&path, false);
    step_until(&mut h, "b shown", |h| {
        h.state().tabs().len() == 2 && item(h, "Hole").is_some()
    });
    meshes_settled_in(&mut h, 1);
    step(&mut h);
    assert!(
        h.state().tabs()[0].view.linked.is_some(),
        "a is hidden with its feature hovered"
    );
    assert!(tinted(&record).is_empty(), "{:?}", tinted(&record));
    hover_item(&mut h, "Block");
    let blocks: Vec<_> = view(&h).bodies()[&1]
        .faces
        .iter()
        .zip(1u32..)
        .filter(|(face, _)| face.name.feature().as_str() == "k7f2qx3m")
        .map(|(_, n)| (1, silico_app::renderer::Element::Face(n)))
        .collect();
    assert!(!blocks.is_empty());
    assert_eq!(tinted(&record), blocks);
    let from = record.lock().unwrap().frames.len();
    h.get_by_label("a.silico").click();
    step_until(&mut h, "a shown", |h| item(h, "Large").is_some());
    meshes_settled(&mut h);
    step(&mut h);
    let told = record.lock().unwrap();
    assert!(told.refused.is_empty(), "{:?}", told.refused);
    for frame in &told.frames[from..] {
        assert!(frame.tinted.is_empty(), "{:?}", frame.tinted);
    }
    drop(told);
    assert_eq!(h.state().tabs()[0].view.linked, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_canvas_hover_in_one_tab_rings_no_item_of_another_with_the_same_id() {
    // a's large cube (a91cw0de) hovered in a's canvas: its item ringed. b opened over it,
    // the pointer left on the canvas, every pick still finding body 2's first face: b has
    // no body 2, so b hovers nothing and rings no item, its pin (a91cw0de too) least of
    // all, in any frame. The picks then finding b's body's first face: the feature that
    // made it ringed, b's own.
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    finding(&record, Some((2, Face(1))));
    let at = the_viewport(&h).center();
    h.hover_at(at);
    step_until(&mut h, "a's hover", |h| ringed(h, 0).is_some());
    assert_eq!(ringed(&h, 0).as_deref(), Some("a91cw0de"));
    let path = f.file("b.silico", EXAMPLE);
    h.state_mut().open(&path, false);
    let deadline = std::time::Instant::now() + WAIT;
    let mut frames = 0;
    while !(h.state().tabs().len() == 2 && item(&h, "Hole").is_some()) || frames < 30 {
        step(&mut h);
        frames += 1;
        if h.state().tabs().len() == 2 {
            assert_eq!(ringed(&h, 1), None, "frame {frames}");
            assert!(h.state().tabs()[1].selection().hovered().is_none());
        }
        assert!(std::time::Instant::now() < deadline, "b never shown");
        std::thread::sleep(Duration::from_millis(5));
    }
    meshes_settled_in(&mut h, 1);
    let made = view(&h).bodies()[&1].faces[0]
        .name
        .feature()
        .as_str()
        .to_owned();
    finding(&record, Some((1, Face(1))));
    // A pixel away, so the hover is asked again.
    h.hover_at(at + vec2(1.0, 0.0));
    step_until(&mut h, "b's hover", |h| ringed(h, 1).is_some());
    assert_eq!(ringed(&h, 1), Some(made));
    assert!(record.lock().unwrap().refused.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_stale_or_nameless_pick_rings_nothing_and_a_feature_gone_on_disk_lets_its_ring_go() {
    // Picks finding a face its mesh hasn't (the 99th of a box's six), or a body that isn't
    // there, name nothing: no hover, no ring, no frame refused. Then the large cube's face
    // hovered and ringed, and the cube taken out of the file under the pointer, the picks
    // still finding body 2: the ring goes with the hover, and nothing is listed of a body
    // not drawn.
    use silico_app::renderer::Element::{Edge, Face};
    let f = watching();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    let at = the_viewport(&h).center();
    for (k, hit) in [(1, Face(99)), (3, Face(1)), (1, Edge(99)), (0, Face(1))]
        .into_iter()
        .enumerate()
    {
        finding(&record, Some(hit));
        h.hover_at(at + vec2(k as f32, 0.0));
        for _ in 0..6 {
            step(&mut h);
            assert_eq!(ringed(&h, 0), None, "{hit:?}");
            assert!(
                h.state().tabs()[0].selection().hovered().is_none(),
                "{hit:?}"
            );
        }
    }
    finding(&record, Some((2, Face(1))));
    h.hover_at(at + vec2(10.0, 0.0));
    step_until(&mut h, "the large cube ringed", |h| {
        ringed(h, 0).as_deref() == Some("a91cw0de")
    });
    f.file("boxes.silico", SMALL_ALONE);
    step_until(&mut h, "the large cube gone and its ring", |h| {
        item(h, "Large").is_none() && view(h).bodies().len() == 1 && ringed(h, 0).is_none()
    });
    for _ in 0..6 {
        step(&mut h);
        assert_eq!(ringed(&h, 0), None);
    }
    let told = record.lock().unwrap();
    assert!(told.refused.is_empty(), "{:?}", told.refused);
    assert!(told.frames.last().unwrap().hovered.is_empty());
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

/// `n` boxes, `b0000001` on, named `Box001` on, each a body of its own.
fn many_boxes(n: usize) -> String {
    let features: Vec<String> = (1..=n)
        .map(|k| {
            format!(
                r#"{{"id": "b{k:07}", "type": "box", "v": 1, "name": "Box{k:03}", "position": [{x}, 0, 0], "length": 10, "width": 10, "height": 10}}"#,
                x = 20 * k
            )
        })
        .collect();
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

#[test]
fn across_many_features_each_frame_tints_the_one_under_the_pointer_in_that_frame() {
    // 150 boxes, more than the track shows: each item the track shows hovered one frame
    // after another, left to right and back, every frame tinting exactly the faces of the
    // box under the pointer in it (the timeline draws before the canvas), none refused.
    let f = fixture();
    let text = many_boxes(150);
    let (mut h, record) = f.drawn("many.silico", &text, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Box001").is_some());
    // Clear of the summary at the track's right ("150 features", under 150 points).
    let right = h
        .get_by_role_and_label(egui::accesskit::Role::Pane, "Timeline")
        .rect()
        .right()
        - 150.0;
    let shown: Vec<(String, egui::Pos2)> = (1..=150)
        .filter_map(|k| {
            let name = format!("Box{k:03}");
            let rect = item(&h, &name)?.rect();
            (rect.right() <= right).then(|| (name, rect.center()))
        })
        .collect();
    assert!(shown.len() > 20, "{} shown", shown.len());
    let bodies: std::collections::BTreeMap<String, u32> = shown
        .iter()
        .map(|(name, _)| {
            let k: usize = name[3..].parse().unwrap();
            (name.clone(), body_of(&h, &format!("b{k:07}")))
        })
        .collect();
    for (name, at) in shown.iter().chain(shown.iter().rev()) {
        h.hover_at(*at);
        step(&mut h);
        assert_eq!(tinted(&record), box_faces(bodies[name]), "{name}");
    }
    assert!(record.lock().unwrap().refused.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tab_shown_again_rings_no_item_for_a_hover_it_had_before_it_was_hidden() {
    // a's large cube hovered in its canvas and ringed; b opened over it with the pointer
    // still on the canvas, so a is hidden hovering, its canvas never drawn to let the hover
    // go. The pointer then onto the app bar, and a's tab clicked: a's canvas isn't under
    // the pointer, so from the frame a is shown again no item of its is ringed, nor its
    // hover of before said.
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    finding(&record, Some((2, Face(1))));
    h.hover_at(the_viewport(&h).center());
    step_until(&mut h, "a's hover", |h| ringed(h, 0).is_some());
    let path = f.file("b.silico", EXAMPLE);
    h.state_mut().open(&path, false);
    step_until(&mut h, "b shown", |h| {
        h.state().tabs().len() == 2 && item(h, "Hole").is_some()
    });
    meshes_settled_in(&mut h, 1);
    finding(&record, None);
    let tab = h.get_by_label("a.silico").rect().center();
    h.hover_at(tab);
    step(&mut h);
    step(&mut h);
    h.get_by_label("a.silico").click();
    for frame in 0..10 {
        step(&mut h);
        let shown = item(&h, "Large").is_some();
        if shown {
            assert_eq!(ringed(&h, 0), None, "frame {frame} of a shown again");
            assert!(
                h.state().tabs()[0].selection().hovered().is_none(),
                "frame {frame}"
            );
        }
    }
    assert!(item(&h, "Large").is_some(), "a not shown");
    assert!(record.lock().unwrap().refused.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tab_shown_again_under_a_still_pointer_asks_its_hover_again() {
    // a's large cube hovered in its canvas and ringed; b opened over it, the pointer left
    // there, so a, hidden, lets its hover go, and the pixel it last asked with it. b closed
    // (Ctrl+W), a is shown again under the same still pointer: its hover is asked again and
    // found, its item ringed, though the pointer never moved.
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    finding(&record, Some((2, Face(1))));
    h.hover_at(the_viewport(&h).center());
    step_until(&mut h, "a's hover", |h| ringed(h, 0).is_some());
    let path = f.file("b.silico", EXAMPLE);
    h.state_mut().open(&path, false);
    step_until(&mut h, "b shown", |h| {
        h.state().tabs().len() == 2 && item(h, "Hole").is_some()
    });
    meshes_settled_in(&mut h, 1);
    for _ in 0..5 {
        step(&mut h);
    }
    assert!(h.state().tabs()[0].selection().hovered().is_none());
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::W);
    step_until(&mut h, "b closed", |h| h.state().tabs().len() == 1);
    meshes_settled(&mut h);
    step_until(
        &mut h,
        "a's hover asked again under the still pointer",
        |h| ringed(h, 0).as_deref() == Some("a91cw0de"),
    );
    assert!(record.lock().unwrap().refused.is_empty());
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tab_shown_again_outlines_no_hover_it_had_before_it_was_hidden() {
    // As the ring's case above: a's large cube hovered in its canvas, outlined (its face in
    // the frame's hovered list); b opened over it, the pointer left on the canvas; then the
    // pointer onto a's tab and a click there. No frame drawing a's bodies again (both
    // cubes, bodies 1 and 2; b's is one body) outlines the hover it had before it was
    // hidden.
    use silico_app::renderer::Element::Face;
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    finding(&record, Some((2, Face(1))));
    h.hover_at(the_viewport(&h).center());
    step_until(&mut h, "a's hover outlined", |h| {
        h.state().tabs()[0].selection().hovered().is_some()
    });
    step(&mut h);
    assert_eq!(
        record.lock().unwrap().frames.last().unwrap().hovered,
        [(2, Face(1))]
    );
    let path = f.file("b.silico", EXAMPLE);
    h.state_mut().open(&path, false);
    step_until(&mut h, "b shown", |h| {
        h.state().tabs().len() == 2 && item(h, "Hole").is_some()
    });
    meshes_settled_in(&mut h, 1);
    finding(&record, None);
    let tab = h.get_by_label("a.silico").rect().center();
    h.hover_at(tab);
    step(&mut h);
    step(&mut h);
    let from = record.lock().unwrap().frames.len();
    h.get_by_label("a.silico").click();
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(item(&h, "Large").is_some(), "a not shown");
    let told = record.lock().unwrap();
    let shown: Vec<_> = told.frames[from..]
        .iter()
        .filter(|frame| frame.bodies == [1, 2])
        .collect();
    assert!(!shown.is_empty());
    for frame in shown {
        assert!(frame.hovered.is_empty(), "{:?}", frame.hovered);
    }
    assert!(told.refused.is_empty(), "{:?}", told.refused);
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_feature_deselected_lets_its_tint_go() {
    // The large cube selected by its timeline item, the pointer then on the app bar's tab:
    // its six faces tinted. The selection cleared: none, the very next frames.
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    let large = body_of(&h, "a91cw0de");
    ui::select(&mut h, "Large");
    let tab = h.get_by_label("a.silico").rect().center();
    h.hover_at(tab);
    step(&mut h);
    step(&mut h);
    assert_eq!(tinted(&record), box_faces(large));
    h.state_mut().tabs_mut()[0].selection_mut().clear();
    step(&mut h);
    step(&mut h);
    assert!(h.state().tabs()[0].selection().is_empty());
    assert!(tinted(&record).is_empty(), "{:?}", tinted(&record));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_click_on_empty_canvas_after_a_feature_selected_lets_its_tint_go() {
    // The large cube selected by its timeline item, tinted; a click on the canvas where a
    // pick finds nothing lets the selection go (SolidWorks' click on empty space), and the
    // tint with it.
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its items", |h| item(h, "Large").is_some());
    let large = body_of(&h, "a91cw0de");
    ui::select(&mut h, "Large");
    let tab = h.get_by_label("a.silico").rect().center();
    h.hover_at(tab);
    step(&mut h);
    step(&mut h);
    assert_eq!(tinted(&record), box_faces(large));
    finding(&record, None);
    let centre = the_viewport(&h).center();
    ui::click_canvas(&mut h, centre, false);
    step_until(&mut h, "the selection let go", |h| {
        h.state().tabs()[0].selection().is_empty()
    });
    step(&mut h);
    step(&mut h);
    assert!(tinted(&record).is_empty(), "{:?}", tinted(&record));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_feature_selected_across_a_reload_that_renumbers_its_body_is_tinted_by_each_frames_table() {
    // The large cube alone (body 1) selected by its timeline item, the pointer on the tab;
    // then the small cube written back before it on disk, so the large one is body 2. Every
    // frame drawing the new table (bodies 1 and 2) tints body 2's faces, the very first too:
    // never body 1's, the small cube's, by the numbers of the table before it.
    let f = watching();
    let (mut h, record) = f.drawn("boxes.silico", LARGE_ALONE, (1280.0, 800.0));
    meshes_settled(&mut h);
    step_until(&mut h, "its item", |h| item(h, "Large").is_some());
    assert_eq!(body_of(&h, "a91cw0de"), 1);
    ui::select(&mut h, "Large");
    let tab = h.get_by_label("boxes.silico").rect().center();
    h.hover_at(tab);
    step(&mut h);
    step(&mut h);
    assert_eq!(tinted(&record), box_faces(1));
    let from = record.lock().unwrap().frames.len();
    f.file("boxes.silico", TWO_BOXES);
    step_until(&mut h, "the small cube back", |h| {
        item(h, "Small").is_some() && view(h).bodies().len() == 2
    });
    meshes_settled(&mut h);
    step(&mut h);
    assert_eq!(body_of(&h, "a91cw0de"), 2);
    let told = record.lock().unwrap();
    assert!(told.refused.is_empty(), "{:?}", told.refused);
    let mut renumbered = 0;
    for frame in &told.frames[from..] {
        let want = match frame.bodies.as_slice() {
            [] => Vec::new(),
            [1] => box_faces(1),
            [1, 2] => {
                renumbered += 1;
                box_faces(2)
            }
            other => panic!("bodies {other:?}"),
        };
        assert_eq!(frame.tinted, want, "of {:?}", frame.bodies);
    }
    assert!(renumbered > 0);
    drop(told);
    assert_eq!(never_waits::waits(), 0);
}
