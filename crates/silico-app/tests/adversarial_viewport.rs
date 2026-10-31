//! Hostile input for the viewport (M5.6): the camera controls given non-finite, degenerate,
//! and extreme numbers; the `meshes` read cycle at the edges of `serves`, refused, and
//! interrupted (a tab switched, a session closed, a body renumbered, a revision changed,
//! all mid-read); a display past 4,096 pixels; and pointer events out of order. Driven as
//! the app's own tests drive it, through `egui_kittest` on the fake kernel, every frame on
//! a thread marked so that a wait on a command fails. Expected values by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use silico_app::App;
use silico_app::navigate;
use silico_app::renderer::{Change, FrameData, Picking, Present, ViewRenderer};
use silico_app::viewport::{View, read_size};
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

const WAIT: Duration = Duration::from_secs(60);

type Fake = Arc<FakeKernel>;
type AppHarness = Harness<'static, App<Fake>>;

struct Fixture {
    dir: TempDir,
    kernel: Fake,
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
}

type Record = Arc<Mutex<Told>>;

struct FakeRenderer(Record);

impl ViewRenderer for FakeRenderer {
    type Failure = String;
    fn upload(&mut self, body: u32, mesh: &Arc<Mesh>) -> Result<(), String> {
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
                told.frames.push(frame);
            }
            None => told.empty += 1,
        }
    }
    fn picked(&mut self) -> Option<Picking> {
        None
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
        let path = self.file(name, text);
        h.state_mut().open(&path, false);
        (h, record)
    }
}

fn step(h: &mut AppHarness) {
    let _marked = never_waits::marked();
    h.step();
}

#[track_caller]
fn step_until(h: &mut AppHarness, what: &str, done: impl Fn(&AppHarness) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(h);
        if done(h) {
            return;
        }
        assert!(Instant::now() < deadline, "never {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn view(h: &AppHarness) -> &View {
    &h.state().tabs()[0].view
}

fn view_mut(h: &mut AppHarness) -> &mut View {
    &mut h.state_mut().tabs_mut()[0].view
}

/// Frames until the shown tab's read has answered with its published model and nothing more
/// is read for three frames.
fn meshes_settled(h: &mut AppHarness, tab: usize) {
    step_until(h, "the meshes read", |h| {
        h.state().tabs().get(tab).is_some_and(|tab| {
            !tab.view.reading() && tab.published.is_some() && tab.view.model() == tab.published
        })
    });
    for _ in 0..3 {
        step(h);
    }
    step_until(h, "the meshes read", |h| {
        !h.state().tabs()[tab].view.reading()
    });
}

fn tessellated(f: &Fixture) -> usize {
    f.kernel
        .calls()
        .iter()
        .filter(|call| matches!(call.op, Op::Tessellate { .. }))
        .count()
}

fn rect(h: &AppHarness) -> egui::Rect {
    view(h).frame().unwrap().rect
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
            assert!(fitted.check().is_ok(), "{width} by {height}");
        }
        let named = navigate::named(navigate::HOME, cube, width, height).unwrap();
        assert!(named.check().is_ok(), "{width} by {height}");
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
                assert!(next.check().is_ok());
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
        assert!(over.check().is_ok());
        assert!((over.eye[2] - 100.0).abs() < 1e-12 && over.eye[0].abs() < 1e-12);
        // Turned about z there, the view spins about its own axis, still valid.
        let spun = navigate::orbit(&over, 90.0, 0.0).unwrap();
        assert!(spun.check().is_ok());
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
            .check()
            .is_ok()
    );
    assert!(navigate::orbit(&twice, 50.0, 50.0).unwrap().check().is_ok());
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
    meshes_settled(&mut h, 0);
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
        meshes_settled(&mut h, 0);
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
        meshes_settled(&mut h, 0);
        assert_eq!(reads_over(&mut h, 100), 0, "eye {eye:?}");
    }
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_read_refused_while_the_kernel_fails_is_asked_again_once_it_mends() {
    // The kernel refuses to mesh while `failing`: the zoomed read is refused. Once it
    // mends, the view, unmoved, is read again of its own accord, and the small cube shown
    // at its level.
    // BREAKS (fixed): a refused read was asked again only when the revision changed or the
    // view moved, so a passing fault left the view coarse until the person moved it.
    let f = fixture();
    let failing = Arc::new(AtomicBool::new(false));
    let refusing = Arc::clone(&failing);
    f.kernel.refuse(move |op| {
        (matches!(op, Op::Tessellate { .. }) && refusing.load(Ordering::SeqCst))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the worker crashed meshing"))
    });
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h, 0);
    let height = f64::from(view(&h).frame().unwrap().size_px[1]);
    failing.store(true, Ordering::SeqCst);
    let reads = view(&h).reads();
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic {
            view_height: 0.08 * height,
        },
        eye: [25.0, -500.0, 25.0],
        target: [25.0, 25.0, 25.0],
        up: [0.0, 0.0, 1.0],
    });
    step_until(&mut h, "the refused read", |h| {
        view(h).reads() > reads && !view(h).reading()
    });
    assert_ne!(view(&h).bodies()[&1].level, 0.03125);
    let message = h.state().tabs()[0].message.clone().unwrap_or_default();
    assert!(
        message.starts_with("The view's meshes couldn't be read: the worker crashed meshing"),
        "{message}"
    );
    // Refused again and again over a hundred frames: not a read each frame, but after a
    // quarter of a second, then a half, and so on.
    assert!(reads_over(&mut h, 100) <= 2);
    failing.store(false, Ordering::SeqCst);
    step_until(&mut h, "the read asked again", |h| {
        view(h)
            .bodies()
            .get(&1)
            .is_some_and(|body| body.level == 0.03125)
    });
    // Answered: the refusal it said is gone.
    assert_eq!(h.state().tabs()[0].message, None);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_tab_switched_mid_read_takes_its_answer_when_shown_again_and_the_renderer_follows() {
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h, 0);
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
    meshes_settled(&mut h, 1);
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
    meshes_settled(&mut h, 0);
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
    meshes_settled(&mut h, 0);
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
    meshes_settled(&mut h, 0);
    let large = Arc::clone(&view(&h).bodies()[&2].mesh);
    let made = tessellated(&f);
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
    assert_eq!(tessellated(&f), made);
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
    meshes_settled(&mut h, 0);
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
    step_until(&mut h, "the first edit's read held", |h| view(h).reading());
    set(&f, b"70");
    let newest = f.sessions.status(&id).session.unwrap().revision;
    step_until(&mut h, "the second edit published", |h| {
        h.state().tabs()[0].published == Some(newest) && view(h).reading()
    });
    hold.release();
    meshes_settled(&mut h, 0);
    // The held read, then one of the newest revision: two, and the bodies its.
    assert_eq!(view(&h).reads(), reads + 2);
    assert_eq!(view(&h).model(), h.state().tabs()[0].published);
    let bounds = view(&h).bodies()[&1].bounds.unwrap();
    assert_eq!(bounds[1][0], 70.0);
    assert_eq!(never_waits::waits(), 0);
}

// ---- Pointer events out of order ----

fn button(h: &AppHarness, at: Pos2, which: PointerButton, pressed: bool) {
    h.event(Event::PointerButton {
        pos: at,
        button: which,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn a_release_with_no_press_and_a_drag_out_of_the_pane_move_the_view_only_while_held() {
    let f = fixture();
    let (mut h, _record) = f.drawn("a.silico", EXAMPLE, (1280.0, 800.0));
    meshes_settled(&mut h, 0);
    let home = view(&h).camera().clone();
    let viewport = rect(&h);
    let at = viewport.center() - vec2(100.0, 0.0);
    // Releases with no press, of every button, and a primary drag with no tool chosen.
    h.hover_at(at);
    for which in [
        PointerButton::Middle,
        PointerButton::Primary,
        PointerButton::Secondary,
    ] {
        button(&h, at, which, false);
    }
    button(&h, at, PointerButton::Primary, true);
    h.hover_at(at + vec2(50.0, 50.0));
    button(&h, at + vec2(50.0, 50.0), PointerButton::Primary, false);
    step(&mut h);
    assert_eq!(view(&h).camera(), &home);
    // The middle button pressed in the viewport, dragged out of it over the browser, and
    // let go there: a pan by the whole way, then nothing as the pointer comes back.
    let out = egui::pos2(viewport.left() - 60.0, at.y);
    h.hover_at(at);
    button(&h, at, PointerButton::Middle, true);
    h.hover_at(out);
    button(&h, out, PointerButton::Middle, false);
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
    button(&h, cube, PointerButton::Middle, true);
    h.hover_at(cube - vec2(80.0, 0.0));
    button(&h, cube - vec2(80.0, 0.0), PointerButton::Middle, false);
    step(&mut h);
    assert_eq!(view(&h).camera(), &panned);
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn a_window_too_small_for_a_viewport_draws_on_and_reads_nothing_wrong() {
    // 60 by 40 points: the panes leave the viewport a few pixels or none.
    let f = fixture();
    let (mut h, record) = f.drawn("a.silico", EXAMPLE, (60.0, 40.0));
    meshes_settled(&mut h, 0);
    assert!(reads_over(&mut h, 100) == 0);
    let frame = record.lock().unwrap().frames.last().unwrap().clone();
    assert!(frame.camera.check().is_ok());
    assert_eq!(frame.bodies, [1]);
    // A wheel there: refused or a valid camera, never NaN.
    h.hover_at(rect(&h).center());
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 200.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    step(&mut h);
    assert!(view(&h).camera().check().is_ok());
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
    meshes_settled(&mut h, 0);
    let size = view(&h).frame().unwrap().size_px;
    assert!(size[0] > MAX_RENDER_SIDE, "{size:?}");
    assert_eq!(reads_over(&mut h, 100), 0);
    // Zoomed at 2×: the same.
    h.set_pixels_per_point(2.0);
    meshes_settled(&mut h, 0);
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
        meshes_settled(&mut h, 0);
        let at = rect(&h).center() - vec2(100.0, 0.0);
        h.hover_at(at);
        button(&h, at, PointerButton::Middle, true);
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
    meshes_settled(&mut h, 0);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: [2500.0, -400.0, 500.0],
        target: [2500.0, 500.0, 500.0],
        up: [0.0, 0.0, 1.0],
    });
    meshes_settled(&mut h, 0);
    let reads = view(&h).reads();
    let centre = silico_app::viewport::view_cube_rect(rect(&h)).center();
    h.hover_at(centre);
    button(&h, centre, PointerButton::Primary, true);
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
    button(&h, centre + vec2(90.0, 0.0), PointerButton::Primary, false);
    meshes_settled(&mut h, 0);
    assert_eq!(view(&h).reads(), reads + 1);
    assert_eq!(never_waits::waits(), 0);
}

/// A present whose picking answers wait until let go, so a test can choose the frame the
/// answer comes in.
struct HeldPicks(Arc<Mutex<(Option<Picking>, bool)>>);

impl Present for HeldPicks {
    fn present(&mut self, _: &egui::Painter, _: egui::Rect, _: Vec<Change>, f: Option<FrameData>) {
        let mut held = self.0.lock().unwrap();
        if let Some(rect) = f.and_then(|frame| frame.picking) {
            held.0 = Some(Picking {
                frame: 1,
                rect,
                hit: None,
            });
        }
    }
    fn picked(&mut self) -> Option<Picking> {
        let mut held = self.0.lock().unwrap();
        if held.1 { held.0.take() } else { None }
    }
    fn refused(&mut self) -> std::collections::BTreeSet<u32> {
        std::collections::BTreeSet::new()
    }
}

#[test]
fn a_pick_answered_after_the_bodies_were_read_again_is_dropped() {
    // The pick is asked of one bodies table; the read under way answers, and the table is
    // made again, in the frame its answer comes: the answer may name bodies renumbered, so
    // it isn't taken.
    let f = fixture();
    let held = Arc::new(Mutex::new((None, false)));
    let app = App::new(Arc::clone(&f.sessions)).presenting(Box::new(HeldPicks(Arc::clone(&held))));
    let mut h = {
        let _marked = never_waits::marked();
        Harness::builder()
            .with_size((1280.0, 800.0))
            .build_ui_state(|ui, app: &mut App<Fake>| app.frame(ui), app)
    };
    let path = f.file("a.silico", EXAMPLE);
    h.state_mut().open(&path, false);
    meshes_settled(&mut h, 0);
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
    // The read's answer readied, and the pick's let go: one frame takes both, the table's
    // first.
    hold.release();
    assert!(f.kernel.wait_idle(WAIT));
    std::thread::sleep(Duration::from_millis(200));
    held.lock().unwrap().1 = true;
    step(&mut h);
    assert!(!view(&h).reading(), "the read's answer came in that frame");
    assert_eq!(view(&h).picked(), None);
    assert!(held.lock().unwrap().0.is_none(), "the answer was given");
    // A pick asked of the table now is taken.
    held.lock().unwrap().1 = false;
    assert!(view_mut(&mut h).pick(rect_px));
    step(&mut h);
    held.lock().unwrap().1 = true;
    step(&mut h);
    assert_eq!(view(&h).picked().map(|picking| picking.rect), Some(rect_px));
    assert_eq!(never_waits::waits(), 0);
}

#[test]
fn highlights_of_a_body_not_drawn_or_an_element_its_mesh_hasnt_are_left_out_of_the_frame() {
    // The renderer refuses a whole frame for one: the large cube hidden, a highlight of its
    // face, and the small cube's 99th face (a box has 6), dropped; the small cube's first
    // face and twelfth edge kept.
    let f = fixture();
    let (mut h, record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h, 0);
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
    // farthest, √(2975² + 975² + 975²) = 3,278.97 mm away, so no view under
    // 3,278.97 · 1e-5 · 1.01 = 0.033118 mm is drawn: the wheel stops there, far above the
    // camera's own least, 1e-6 mm.
    let f = fixture();
    let (mut h, _record) = f.drawn("boxes.silico", TWO_BOXES, (1280.0, 800.0));
    meshes_settled(&mut h, 0);
    view_mut(&mut h).set_camera(Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: [25.0, -500.0, 25.0],
        target: [25.0; 3],
        up: [0.0, 0.0, 1.0],
    });
    step(&mut h);
    let reach = (2975.0f64 * 2975.0 + 975.0 * 975.0 * 2.0).sqrt();
    let least = reach * 1e-5 * 1.01;
    let centre = rect(&h).center();
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
    let height = view_height(view(&h).camera());
    // 80 halved down to the last that's at least 0.033118: 80 / 2¹¹ = 0.039.
    assert!(height >= least && height / 2.0 < least, "{height}");
    assert_eq!(height, 80.0 / 2048.0);
    assert_eq!(never_waits::waits(), 0);
}
