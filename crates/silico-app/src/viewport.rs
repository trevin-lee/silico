//! The viewport (docs/design/m5.0-app.md, sections 2, 3, 8, and 10; M5.6): the canvas that
//! draws the tab's published model through a renderer ([`crate::renderer`]), under the tab's
//! own camera (decision 29), with Fusion's navigation (the wheel, the middle button, F6),
//! its ViewCube at the canvas's top right, and its navigation bar at its bottom centre.
//!
//! Each tab holds its meshes ([`View`]): the newest `meshes` answer's bodies by number, each
//! with its key, its level, its box, the half pixels it serves, its mesh, and its faces' and
//! edges' names (what picking reads, M5.7). It keeps at most one `meshes` read in flight,
//! of the published model, listing every mesh it holds (`held`), and reads again only when
//! the published revision changes, or when the view settles with a body's half pixel
//! outside its `serves`, computed by the core's own rule (`views::half_pixel_at` at
//! `views::meshing_depth`) on the camera the core levels with (rounded as an answer writes
//! it), so no rule is copied here (section 7, `meshes`). A read refused for what it asked
//! isn't asked again until something differs; one refused for the moment (a worker that
//! crashed or timed out, a fault, too many reads at once) is asked again after a delay that
//! doubles. The renderer is told of a body once for each key and level it's meshed at, and
//! of a body gone ([`Drawing`]); each frame it's given the camera, and places the bodies
//! itself (section 10). Nothing here waits: the read is polled with `try_take` (invariant
//! 4).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Layout, PointerButton, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, WidgetInfo,
    WidgetType,
};
use silico_core::ErrorCode;
use silico_core::command::meshes::{Held, MeshElement, MeshedBody, Meshes, MeshesRead};
use silico_core::command::render::AnsweredCamera;
use silico_core::command::{
    ModelChoice, Problems, Response, Revision, Running, SessionId, Sessions,
};
use silico_core::document::FeatureId;
use silico_core::kernel::Clients;
use silico_core::recompute::Key;
use silico_core::views::{NamedView, half_pixel_at, meshing_depth};
use silico_protocol::{Camera, MAX_RENDER_SIDE, Mesh, Projection};

use crate::navigate::{self, HOME, NO_BOX};
use crate::renderer::{Change, Element, FrameData, Picking, PixelRect, Present};
use crate::tab::Tab;
use crate::theme;

/// The first delay before a read refused for the moment is asked again; each refusal after
/// it doubles it, up to [`RETRY_MOST`].
pub const RETRY_FIRST: Duration = Duration::from_millis(250);

/// The longest delay before a read refused for the moment is asked again.
pub const RETRY_MOST: Duration = Duration::from_secs(8);

/// How many frames a pick waits for its answer, which comes a frame after it's asked: past
/// this, it's let go (a renderer that answers none, or none installed), so the frames
/// asked for while one waits stop.
const PICK_FRAMES: u64 = 8;

/// A body whose mesh the tab holds, as the newest `meshes` answer gave it.
#[derive(Debug, Clone)]
pub struct ViewBody {
    pub feature: FeatureId,
    /// Its step's key, and the level and deflection it was meshed at.
    pub key: Key,
    pub level: f64,
    pub deflection: f64,
    /// Its tight box, if it was measured.
    pub bounds: Option<[[f64; 3]; 2]>,
    /// The half pixels `[at_least, below)` its mesh serves.
    pub serves: [f64; 2],
    /// Whether the answer that gave it said the tab held its mesh (and so sent no arrays).
    pub held: bool,
    pub mesh: Arc<Mesh>,
    /// Its faces and edges, in the mesh's order, each named (`Face1` is the first).
    pub faces: Vec<MeshElement>,
    pub edges: Vec<MeshElement>,
}

impl ViewBody {
    /// Whether `element` is one of its mesh's faces or edges.
    fn has(&self, element: Element) -> bool {
        let (number, count) = match element {
            Element::Face(n) => (n, self.mesh.face_triangles.len()),
            Element::Edge(n) => (n, self.mesh.edge_point_counts.len()),
        };
        usize::try_from(number).is_ok_and(|n| (1..=count).contains(&n))
    }
}

/// What the viewport is this frame, for what draws over it and picks in it (sketch mode,
/// M5.7): the tab's session; where it is in the window, in points; the camera; the size in
/// physical pixels; the display's scale; the pointer, in physical pixels from the
/// viewport's top left, if it's over the viewport; and the left button's events and the
/// right button's clicks the pane didn't take itself.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewFrame {
    pub session: SessionId,
    pub rect: Rect,
    pub camera: Camera,
    pub size_px: [u32; 2],
    pub pixels_per_point: f32,
    pub pointer_px: Option<[f32; 2]>,
    /// The left button's events on the canvas, in order, raw: none the ViewCube or the
    /// navigation bar took, and none while a navigation tool makes a left drag navigate.
    /// Who acts on them (sketch mode, the selection) is the tab's mode, not the pane's.
    ///
    /// Every Press is followed by exactly one Release or Cancel, in this frame or a later
    /// one, given to the same tab (a tab no longer shown is given its end there,
    /// [`end_unshown`]), with only Moves between; and nothing is handed on between that
    /// end and the next Press but a DoubleClick, straight after the Release that made it.
    pub left: Vec<LeftEvent>,
    /// The right button's clicks on the canvas, in order ([`SecondaryClick`]): none the
    /// ViewCube (whose right click opens its menu) or the navigation bar took, and none
    /// pressed while the pane drags (the middle button's pan or orbit, a navigation tool's
    /// left drag, the cube's orbit). No navigation takes the right button (Fusion's preset:
    /// the middle button and the wheel, and a tool the left drag), so a navigation tool
    /// chosen takes none of its clicks. What a click does is the tab's mode's, and so is
    /// the menu it opens, if any (sketch mode's End chain, Finish, and Cancel): the pane
    /// opens none on the canvas.
    ///
    /// A click's `seq` places it among the frame's left events: a slow frame's left click
    /// and the right click after it (the Line tool's click, then End chain) are told apart.
    pub secondary: Vec<SecondaryClick>,
}

/// What a left-button event was. A Press begins a stream, which ends in exactly one Release
/// or Cancel ([`ViewFrame::left`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftKind {
    /// Pressed on the canvas.
    Press,
    /// Moved while held, from a press on the canvas.
    Move,
    /// Let go, after a press on the canvas.
    Release,
    /// Held no longer, with no release: the window lost the focus, a navigation tool took
    /// the drag, another press came with no release before it (the window lost one), or
    /// the tab stopped being shown. What the stream did is undone, not done; it's at where
    /// the stream was last (its press, or its last move). The pointer leaving the window
    /// cancels nothing: the drag goes on past its edge, and its release, wherever it
    /// comes, ends it.
    Cancel,
    /// A double-click on the canvas, straight after the Release that made it: the second of
    /// two left clicks within egui's delay and distance, counted as egui counts them
    /// (a third quick one is no double-click), of the left button's clicks alone (egui
    /// counts every button's together, so a right click and a left one would be its
    /// double-click).
    DoubleClick,
}

/// A left-button event on the canvas: what it was, where (physical pixels from the
/// viewport's top left), the modifiers held (Ctrl is Cmd on macOS, egui's `command`), the
/// frame it came in (the pane's frame number, [`crate::renderer::Frame::serial`]'s), and
/// egui's input time, in seconds, so a consumer's double-clicks and drag thresholds are
/// the same in a test as in a window.
///
/// `at_px` is in physical pixels, not points, and may lie outside the viewport (a drag
/// let go past its edge, the window's other panes): a consumer clamps it to the size
/// before it makes a [`PixelRect`] of it. Every event of one frame has the frame's one
/// `time`: egui gives its events no time of their own, so `seq` orders them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeftEvent {
    pub kind: LeftKind,
    pub at_px: [f32; 2],
    pub shift: bool,
    pub ctrl: bool,
    pub frame: u64,
    pub time: f64,
    /// Its place among the frame's left events and right clicks together
    /// ([`ViewFrame::left`] and [`ViewFrame::secondary`]), from 0, each once, in the order
    /// egui gave the window's events: a Cancel a navigation tool forces first, a Cancel
    /// just before the Press that forced it, a DoubleClick just after its Release.
    pub seq: u32,
}

/// A right click on the canvas ([`ViewFrame::secondary`]): a press of the right button on
/// it and its release, the pointer kept within egui's click distance of the press
/// (`max_click_dist`, in points) and the button let go within its click duration
/// (`max_click_duration`), as egui judges a click for `secondary_clicked`; and no other
/// button pressed or let go between (egui gives the click to the widget pressed, and any
/// button's release ends that). A right drag is never one, nor is a press in another tab
/// let go in this one.
///
/// It's handed on once, at its release: where it was let go (physical pixels from the
/// viewport's top left, as [`LeftEvent::at_px`], and so up to the click distance outside
/// the viewport for a press at its edge), the modifiers held then (Ctrl is Cmd on macOS,
/// egui's `command`), the frame it came in, and egui's input time, as a [`LeftEvent`]'s.
/// Several in one frame are handed on in their order, each with the frame's one `time`;
/// `seq` orders them among the frame's left events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SecondaryClick {
    pub at_px: [f32; 2],
    pub shift: bool,
    pub ctrl: bool,
    pub frame: u64,
    pub time: f64,
    /// Its place among the frame's left events and right clicks together, as
    /// [`LeftEvent::seq`].
    pub seq: u32,
}

/// A press of one button, to judge its release a click as egui does: where and when it
/// was pressed (in points, and egui's input time), and whether the pointer has since gone
/// farther from it than egui's click distance.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pressed {
    origin: Pos2,
    time: f64,
    far: bool,
}

impl Pressed {
    fn new(origin: Pos2, time: f64) -> Self {
        Pressed {
            origin,
            time,
            far: false,
        }
    }

    fn moved(&mut self, to: Pos2, options: &egui::InputOptions) {
        self.far |= self.origin.distance(to) > options.max_click_dist;
    }

    /// Whether its release at `at`, at `time`, is a click: near enough, soon enough.
    fn clicked(mut self, at: Pos2, time: f64, options: &egui::InputOptions) -> bool {
        self.moved(at, options);
        !self.far && time - self.time <= options.max_click_duration
    }
}

/// The left button's clicks anywhere in the window, counted as egui counts clicks, but the
/// left button's alone: its press under way, and its last two clicks, when and where.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct LeftClicks {
    pressed: Option<Pressed>,
    last: Option<(f64, Pos2)>,
    last_last: Option<f64>,
}

impl LeftClicks {
    /// Its release at `at`, at `time`: whether it's a double-click, egui's count of two
    /// (a third quick click is a triple, no double).
    fn released(&mut self, at: Pos2, time: f64, options: &egui::InputOptions) -> bool {
        let Some(pressed) = self.pressed.take() else {
            return false;
        };
        if !pressed.clicked(at, time, options) {
            return false;
        }
        let near = self.last.is_none_or(|(_, last)| {
            last.distance_sq(at) < options.max_click_dist * options.max_click_dist
        });
        let delay = options.max_double_click_delay;
        let double = near && self.last.is_some_and(|(last, _)| time - last < delay);
        let triple = near && self.last_last.is_some_and(|last| time - last < delay * 2.0);
        self.last_last = self.last.map(|(last, _)| last);
        self.last = Some((time, at));
        double && !triple
    }
}

/// The right button held from a press on the canvas, kept for the whole pane: the tab it
/// was pressed in, by its session, and the press.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SecondaryHeld {
    session: SessionId,
    pressed: Pressed,
}

/// The left button, kept for the whole pane, not a tab's (the pointer is the window's,
/// whichever tab is shown): held from a press on the canvas, and its clicks anywhere in the
/// window, as egui counts them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct LeftButton {
    held: Option<LeftHeld>,
    clicks: LeftClicks,
}

/// The left button held from a press on the canvas, kept for the whole pane, not a tab's
/// (the pointer is the window's, whichever tab is shown): the tab it was pressed in, by its
/// session, and where its stream was last (its press, or its last move), in physical
/// pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LeftHeld {
    session: SessionId,
    at_px: [f32; 2],
}

/// What a `meshes` read was asked under: the published revision, the camera, and the size.
#[derive(Debug, Clone, PartialEq)]
struct Asked {
    revision: Revision,
    /// The tab's count of the models it has seen published when it was asked
    /// ([`View::published_is`]): a model published again at the same revision is another.
    model: u64,
    camera: Camera,
    size: [u32; 2],
}

/// The `meshes` read in flight.
struct Reading {
    running: Running<MeshesRead>,
    asked: Asked,
    /// Asked for Home's view, fitted by the core, whose camera the tab then takes.
    named: bool,
}

/// What a drag in the viewport does, and the navigation bar's tool, which a left drag is
/// while it's chosen (fusion.md, *ViewCube and Navigation Bar*). With no tool chosen, a
/// left drag is the selection's (M5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Navigation {
    Orbit,
    Pan,
    Zoom,
}

/// A read refused for the moment: the read, when it may be asked again, and how many such
/// refusals of it ran in a row.
#[derive(Debug, Clone)]
struct Retry {
    asked: Asked,
    at: Instant,
    tries: u32,
}

/// A drag under way: on the canvas, by its button, the pointer where it was last frame; or
/// of the ViewCube, which orbits.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    Canvas { button: PointerButton, last: Pos2 },
    Cube,
}

/// What a read's answer means to the tab's message: a refusal to say, or one said that a
/// read has since answered.
enum Notice {
    Refused(String),
    Answered(String),
}

/// A tab's viewport: its camera, its meshes, and its read.
pub struct View {
    camera: Camera,
    /// Whether the view is perspective: kept through Home, the ViewCube, and fits.
    perspective: bool,
    /// Whether the camera was chosen: by the person, or by the first read's Home fit. Until
    /// then, a read asks for Home's view of the bodies, and the tab takes its camera.
    placed: bool,
    /// The bodies by number, as the newest answer gave them, the revision it read, and
    /// what it was asked under; and one more each time the table is made again, which a
    /// pick asked of an older table isn't taken for.
    bodies: BTreeMap<u32, ViewBody>,
    model: Option<Revision>,
    answered: Option<Asked>,
    generation: u64,
    reading: Option<Reading>,
    /// The last read refused for what it asked: not asked again until something differs.
    refused: Option<Asked>,
    /// A read refused for the moment, to ask again after a delay.
    retry: Option<Retry>,
    /// The refusal said in the tab's message; the revision and the words it was said of (a
    /// refusal is said once a revision, not at every retry; another in the same revision
    /// is said too); and the notice for the tab to take.
    said: Option<String>,
    said_for: Option<(Revision, String)>,
    /// The bodies the browser hides, by feature, as the last frame had them: drawn by no
    /// frame, and so not in the reach a view is clamped by.
    hidden: BTreeSet<FeatureId>,
    notice: Option<Notice>,
    /// A held mesh an answer gave none of wasn't found: the next read holds none.
    hold_none: bool,
    reads: u64,
    /// The camera and size of the last frame, to tell when the view has settled.
    last: Option<(Camera, [u32; 2])>,
    drag: Option<Drag>,
    /// The navigation bar's tool, if one is chosen.
    pub tool: Option<Navigation>,
    /// The elements drawn highlighted, set from outside (M5.7's selection): passed through,
    /// but for one of a body the frame doesn't draw, or that its mesh hasn't.
    pub highlights: BTreeSet<(u32, Element)>,
    /// The feature hovered in the timeline (DESIGN.md, *Hover linking*): the faces it made,
    /// as each face's name says ([`Name::feature`]), are highlighted with the selection's
    /// while it's hovered.
    ///
    /// [`Name::feature`]: silico_core::naming::Name::feature
    ///
    /// SEAM: the prototype tints them with the maker's colour,
    /// [`theme::VIEWPORT_FEATURE_HOVER_TINT`] (`#d4dbff` at 0.4), not the selection's; that
    /// comes once the renderer has a second highlight style.
    pub linked: Option<FeatureId>,
    picking: Option<PixelRect>,
    picked: Option<Picking>,
    frame: Option<ViewFrame>,
    /// The pane's height in pixels, as its last frame had it: an orthographic view's least
    /// is the renderer's at that height (`views::least_view_height`). 1 before the first
    /// frame, so a camera clamped then takes the 1-row floor until that frame re-clamps it.
    rows: u32,
    /// A view a shortcut asked for (F6, Home, Ctrl+1 to Ctrl+7: `crate::shortcuts`), given
    /// at the pane's next navigation, which knows the canvas's size.
    asked: Option<ViewAsk>,
    /// The tab's count of published models, as the tab last told it, and as it was when the
    /// bodies held were asked for: a model published again at the same revision (a fresh
    /// check's, Ctrl+Shift+B) makes them stale, as a new revision does.
    published: u64,
    read_published: u64,
}

/// A view a shortcut asks of the tab's viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewAsk {
    /// The bodies fitted, looking as it looks (F6, the navigation bar's Fit).
    Fit,
    /// A named view of the bodies, as the ViewCube gives it (Home, a face, a corner).
    Named(NamedView),
}

impl Default for View {
    fn default() -> Self {
        View {
            camera: HOME.camera(NO_BOX, 1, 1),
            perspective: false,
            placed: false,
            bodies: BTreeMap::new(),
            model: None,
            answered: None,
            generation: 0,
            reading: None,
            refused: None,
            retry: None,
            said: None,
            said_for: None,
            hidden: BTreeSet::new(),
            notice: None,
            hold_none: false,
            reads: 0,
            last: None,
            drag: None,
            tool: None,
            highlights: BTreeSet::new(),
            linked: None,
            picking: None,
            picked: None,
            frame: None,
            rows: 1,
            asked: None,
            published: 0,
            read_published: 0,
        }
    }
}

/// The half pixel, in mm, `camera` gives a body with box `bounds` in an image `height`
/// pixels high: at its meshing depth, or at none with no box, as the read takes it
/// (`command/meshes.rs`, `level_of`), so the app reads again by the core's own rule.
pub fn half_pixel(camera: &Camera, height: u32, bounds: Option<[[f64; 3]; 2]>) -> Option<f64> {
    let depth = bounds.map_or(0.0, |bounds| meshing_depth(camera, bounds));
    half_pixel_at(camera, height, depth)
}

/// Whether any of `bodies` has its half pixel, under `camera` in an image `height` pixels
/// high, outside the half pixels its mesh serves: a read would mesh it otherwise. The
/// camera is taken as a read takes it, rounded as an answer writes it
/// (`AnsweredCamera::of`), which the core levels with: by the unrounded one, a view at a
/// level's edge would read again for ever.
pub fn outside_serves<'a>(
    camera: &Camera,
    height: u32,
    bodies: impl IntoIterator<Item = &'a ViewBody>,
) -> bool {
    let Some(leveled) = AnsweredCamera::of(camera).map(|answered| answered.camera()) else {
        return false;
    };
    bodies.into_iter().any(|body| {
        half_pixel(&leveled, height, body.bounds)
            .is_some_and(|half| !(body.serves[0] <= half && half < body.serves[1]))
    })
}

/// The size a `meshes` read asks for: the viewport's, scaled by one factor so its longer
/// side is at most [`MAX_RENDER_SIDE`], the most a read takes (so a fit, which takes the
/// width over the height, fits the shape shown), each side at least 1.
pub fn read_size(size_px: [u32; 2]) -> [u32; 2] {
    let longer = size_px[0].max(size_px[1]);
    if longer <= MAX_RENDER_SIDE {
        return size_px.map(|side| side.max(1));
    }
    let scale = f64::from(MAX_RENDER_SIDE) / f64::from(longer);
    // At most MAX_RENDER_SIDE, so the cast keeps every value.
    size_px.map(|side| ((f64::from(side) * scale).round() as u32).clamp(1, MAX_RENDER_SIDE))
}

/// Whether a refusal is of what the read asked (its view, its camera, an eye inside a
/// solid, its size, or its features), which the same read would meet again; not of the
/// moment (a worker that crashed or timed out, a fault, too many reads at once), which a
/// read asked again later may not.
fn of_the_request(problems: &Problems) -> bool {
    problems.errors().iter().any(|error| {
        error.code() == ErrorCode::InvalidInput
            && error.parameter().is_some_and(|parameter| {
                ["view", "width", "height"].contains(&parameter)
                    || parameter.starts_with("camera")
                    || parameter.starts_with("features")
            })
    })
}

/// The delay before a read refused for the moment, the `tries`th in a row (from 0), is
/// asked again.
fn retry_after(tries: u32) -> Duration {
    RETRY_FIRST
        .saturating_mul(2u32.saturating_pow(tries))
        .min(RETRY_MOST)
}

impl View {
    /// The camera.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// The tab's count of the models it has seen published (`Tab::published_models`):
    /// once it moves, the bodies read of the model before are stale, whatever its revision.
    pub fn published_is(&mut self, models: u64) {
        self.published = models;
    }

    /// `ask`ed of the view by a shortcut: given at the pane's next navigation, in place of
    /// any asked before it that hasn't been.
    pub fn ask(&mut self, ask: ViewAsk) {
        self.asked = Some(ask);
    }

    /// Looks through `camera`, as a control of the person's does.
    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
        self.placed = true;
    }

    /// The bodies whose meshes the tab holds, by number.
    pub fn bodies(&self) -> &BTreeMap<u32, ViewBody> {
        &self.bodies
    }

    /// The faces the feature hovered in the timeline made ([`linked`](Self::linked)), in
    /// every body that holds one, as each face's name says: none with none hovered.
    pub fn linked_faces(&self) -> Vec<(u32, Element)> {
        let Some(feature) = &self.linked else {
            return Vec::new();
        };
        self.bodies
            .iter()
            .flat_map(|(number, body)| {
                body.faces
                    .iter()
                    .zip(1u32..)
                    .filter(|(face, _)| face.name.feature() == feature)
                    .map(|(_, n)| (*number, Element::Face(n)))
            })
            .collect()
    }

    /// The published revision the bodies were read from, once a read has answered.
    pub fn model(&self) -> Option<Revision> {
        self.model
    }

    /// How many `meshes` reads the tab has started.
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// Whether a `meshes` read is in flight.
    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Whether the view is perspective.
    pub fn perspective(&self) -> bool {
        self.perspective
    }

    /// What the viewport was last frame (M5.7), its `left` and `secondary` empty: the
    /// buttons' events are handed on once, to the tab's overlay ([`Tab::overlay`]), never
    /// read again here.
    pub fn frame(&self) -> Option<&ViewFrame> {
        self.frame.as_ref()
    }

    /// Asks the next frame to pick under `rect`, if a renderer takes it
    /// ([`PixelRect::takes`]): its answer comes a frame after ([`picked`](Self::picked)),
    /// and the last answer is let go now. Whether it was asked.
    pub fn pick(&mut self, rect: PixelRect) -> bool {
        let takes = rect.takes();
        if takes {
            self.picking = Some(rect);
            self.picked = None;
        }
        takes
    }

    /// The answer to the last pick asked, once it has come, if it's of the bodies the tab
    /// holds now.
    pub fn picked(&self) -> Option<Picking> {
        self.picked
    }

    /// The read in flight polled: its answer taken if it has come.
    fn poll(&mut self, now: Instant) {
        let Some(reading) = self.reading.take() else {
            return;
        };
        match reading.running.try_take() {
            Ok(response) => self.took(reading.asked, reading.named, response, now),
            Err(running) => {
                self.reading = Some(Reading { running, ..reading });
            }
        }
    }

    /// A read's answer: its bodies taken (and, for the first, Home's camera); or its
    /// refusal said, and kept so the same read isn't asked again if it's of what was
    /// asked, or asked again after a delay if it's of the moment.
    fn took(&mut self, asked: Asked, named: bool, response: Response<MeshesRead>, now: Instant) {
        match response.result {
            Ok(read) => {
                if named && !self.placed {
                    self.camera = read.camera.camera();
                }
                self.placed = true;
                self.refused = None;
                self.retry = None;
                self.said_for = None;
                if let Some(said) = self.said.take() {
                    self.notice = Some(Notice::Answered(said));
                }
                self.model = Some(
                    response
                        .model
                        .map_or(asked.revision, |model| model.revision),
                );
                self.read_published = asked.model;
                self.answered = Some(asked);
                self.take_bodies(read.bodies);
            }
            // Home fits the bodies by their boxes: with none to fit (no body, or one that
            // couldn't be measured), the tab keeps its camera and reads with it. Never said,
            // even when the person moved the view while it was read: Home was the app's own
            // ask, not theirs.
            Err(problems)
                if named
                    && problems
                        .errors()
                        .iter()
                        .any(|error| error.parameter() == Some("view")) =>
            {
                self.placed = true;
            }
            Err(problems) => {
                let said = format!(
                    "The view's meshes couldn't be read: {}",
                    crate::tab::said(&problems)
                );
                tracing::warn!(revision = asked.revision.number(), "{said}");
                let revision = asked.revision;
                if of_the_request(&problems) {
                    self.retry = None;
                    self.refused = Some(asked);
                } else {
                    // The delay grows with each refusal of the revision in a row, whatever
                    // the view: a view moved on is no reason to ask sooner.
                    let tries = self
                        .retry
                        .as_ref()
                        .filter(|retry| retry.asked.revision == asked.revision)
                        .map_or(0, |retry| retry.tries);
                    self.retry = Some(Retry {
                        asked,
                        at: now + retry_after(tries),
                        tries: tries.saturating_add(1),
                    });
                }
                let saying = Some((revision, said.clone()));
                if self.said_for != saying {
                    self.said_for = saying;
                    self.said = Some(said.clone());
                    self.notice = Some(Notice::Refused(said));
                }
            }
        }
    }

    /// The table made again from an answer's bodies: each body's mesh the answer's, or, for
    /// one held, the one the tab holds of its key at its deflection. A held one the tab
    /// hasn't (which a read never answers) is left out, and the next read holds none.
    fn take_bodies(&mut self, bodies: Vec<MeshedBody>) {
        let held: BTreeMap<(Key, u64), Arc<Mesh>> = self
            .bodies
            .values()
            .map(|body| {
                (
                    (body.key, body.deflection.to_bits()),
                    Arc::clone(&body.mesh),
                )
            })
            .collect();
        let mut table = BTreeMap::new();
        for body in bodies {
            let Some(number) = body.body else {
                continue;
            };
            let mesh = match body.mesh {
                Some(mesh) => mesh,
                None => match held.get(&(body.key, body.deflection.to_bits())) {
                    Some(mesh) => Arc::clone(mesh),
                    None => {
                        self.hold_none = true;
                        self.model = None;
                        self.answered = None;
                        continue;
                    }
                },
            };
            table.insert(
                number,
                ViewBody {
                    feature: body.feature,
                    key: body.key,
                    level: body.level,
                    deflection: body.deflection,
                    bounds: body.bounds,
                    serves: body.serves,
                    held: body.held,
                    mesh,
                    faces: body.faces,
                    edges: body.edges,
                },
            );
        }
        self.bodies = table;
        self.generation = self.generation.wrapping_add(1);
        // A view zoomed in before these bodies came (or as close as smaller ones allowed)
        // raised to the least they allow, so every control can move it on.
        self.camera = navigate::clamped(self.camera.clone(), self.rows, self.drawn_boxes());
    }

    /// Every body's box.
    fn boxes(&self) -> impl Iterator<Item = [[f64; 3]; 2]> + '_ {
        self.bodies.values().filter_map(|body| body.bounds)
    }

    /// The boxes of the bodies drawn (those the browser hides left out), as the renderer
    /// takes their reach: a body's tight box, or, for one that couldn't be measured, its
    /// mesh's own extent.
    fn drawn_boxes(&self) -> impl Iterator<Item = [[f64; 3]; 2]> + '_ {
        self.bodies
            .values()
            .filter(|body| !self.hidden.contains(&body.feature))
            .filter_map(|body| body.bounds.or_else(|| extent(&body.mesh)))
    }

    /// The read to start at `now`, if one is wanted (section 3, *When the app reads*): none
    /// while one is in flight, nothing is published, or a read of this revision, refused for
    /// the moment, waits to be asked again (another revision is read at once); else one when the published revision isn't the one the
    /// bodies were read from, or when the view has `settled` with a body's half pixel
    /// outside its `serves`. Never the read last answered again, nor one refused for what
    /// it asked; after such a refusal of this revision, only once the view has settled.
    fn wanted(
        &self,
        published: Option<Revision>,
        size: [u32; 2],
        settled: bool,
        now: Instant,
    ) -> Option<Asked> {
        if self.reading.is_some() {
            return None;
        }
        let asked = Asked {
            revision: published?,
            model: self.published,
            camera: self.camera.clone(),
            size,
        };
        // A retry waiting holds back every read of its revision, settled or not (a wheel's
        // every notch settles), until its time; then the view as it is then is read. A new
        // revision is read now.
        if let Some(retry) = &self.retry
            && now < retry.at
            && retry.asked.revision == asked.revision
        {
            return None;
        }
        if let Some(refused) = &self.refused
            && refused.revision == asked.revision
            && (!settled || *refused == asked)
        {
            return None;
        }
        if self.answered.as_ref() == Some(&asked) {
            return None;
        }
        let stale = self.model != Some(asked.revision) || self.read_published != asked.model;
        let outside = settled && outside_serves(&self.camera, size[1], self.bodies.values());
        (stale || outside).then_some(asked)
    }

    /// A retry due at `now` that no read was wanted for, the view `settled`: its run ends,
    /// so a later refusal starts again at the shortest delay. A frame the view moves in
    /// wants no read for that alone, and ends nothing, or a run falling due mid-orbit would
    /// never grow past the shortest.
    fn ended_if_due(&mut self, settled: bool, now: Instant) {
        if settled
            && self.reading.is_none()
            && self.retry.as_ref().is_some_and(|retry| retry.at <= now)
        {
            self.retry = None;
        }
    }

    /// How long until a read refused for the moment may be asked again, if one waits: none
    /// once it's due, whether or not a read is then wanted (a frame asked at once for a due
    /// retry no read is wanted for would come at the display's rate for ever).
    fn wake_in(&self, now: Instant) -> Option<Duration> {
        let at = self.retry.as_ref()?.at;
        (now < at).then(|| at - now)
    }

    /// The request for the read `asked`: of Home's view until the camera is placed, else
    /// of the camera; listing every mesh held, unless a held one went missing, once.
    fn request(&mut self, asked: &Asked) -> Meshes {
        let named = !self.placed;
        let held: Vec<Held> = if std::mem::take(&mut self.hold_none) {
            Vec::new()
        } else {
            self.bodies
                .values()
                .map(|body| Held {
                    key: body.key,
                    deflection: body.deflection,
                })
                .collect()
        };
        Meshes {
            view: named.then_some(HOME),
            camera: (!named).then(|| asked.camera.clone()),
            width: asked.size[0],
            height: asked.size[1],
            features: None,
            held,
        }
    }

    /// Starts the read `asked`, of the published model.
    fn start<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        asked: Asked,
    ) {
        let request = self.request(&asked);
        self.reads += 1;
        self.reading = Some(Reading {
            named: request.view.is_some(),
            running: sessions.meshes(session, ModelChoice::Published, request),
            asked,
        });
    }

    /// The camera moved to `camera`, if a control made one, its view raised to the least the
    /// bodies allow if it's under it (`navigate::clamped`). Whether it was taken.
    fn moved(&mut self, camera: Option<Camera>) -> bool {
        let Some(camera) = camera else {
            return false;
        };
        self.camera = navigate::clamped(camera, self.rows, self.drawn_boxes());
        self.placed = true;
        true
    }

    /// Zooms by `scroll` points toward `cursor`, in a view `height` points high: an
    /// orthographic zoom in stops at the least view the bodies allow, the point under the
    /// cursor kept.
    fn zoom(&mut self, scroll: f64, cursor: [f64; 2], height: f64) {
        let mut scroll = scroll;
        if let Projection::Orthographic { view_height } = self.camera.projection {
            // The last frame's rows, not `height` (points): for the one frame after a resize
            // the floor is the old pane's, and the next frame's clamp raises it.
            let least = silico_core::views::least_view_height(
                self.rows,
                silico_core::views::reach(self.camera.target, self.drawn_boxes()),
            );
            // The most it may zoom in: to the least, at 200 points an octave.
            let most = -navigate::ZOOM_POINTS_PER_OCTAVE * libm::log2(least / view_height);
            if scroll > 0.0 && least > 0.0 {
                scroll = scroll.min(most.max(0.0));
            }
        }
        if scroll != 0.0 {
            let camera = navigate::zoom(&self.camera, scroll, cursor, height);
            self.moved(camera);
        }
    }

    /// Every box the tab holds, joined.
    fn bounds(&self) -> Option<[[f64; 3]; 2]> {
        silico_core::views::joined(self.boxes())
    }

    /// The named view `view` of the bodies' boxes (a cube's, with none), in the view's
    /// projection.
    fn look_from(&mut self, view: NamedView, size_px: [u32; 2]) {
        let bounds = self.bounds().unwrap_or(NO_BOX);
        let camera = navigate::named(view, bounds, size_px[0], size_px[1])
            .and_then(|camera| navigate::projected(&camera, self.perspective));
        self.moved(camera);
    }

    /// Fits the view to the bodies' boxes (F6), looking as it looks.
    fn fit(&mut self, size_px: [u32; 2]) {
        if let Some(bounds) = self.bounds() {
            let camera = navigate::fit(&self.camera, bounds, size_px[0], size_px[1]);
            self.moved(camera);
        }
    }

    /// The view made perspective, or orthographic.
    fn project(&mut self, perspective: bool) {
        let camera = navigate::projected(&self.camera, perspective);
        if self.moved(camera) {
            self.perspective = perspective;
        }
    }

    /// A drag of `delta` points, in a view `height` points high, as `navigation` takes it.
    fn navigated(&mut self, navigation: Navigation, delta: Vec2, height: f64) {
        let (dx, dy) = (f64::from(delta.x), f64::from(delta.y));
        let camera = match navigation {
            Navigation::Orbit => navigate::orbit(&self.camera, dx, dy),
            Navigation::Pan => navigate::pan(&self.camera, dx, dy, height),
            // A drag up zooms in, about the view's centre.
            Navigation::Zoom => {
                self.zoom(-dy, [0.0, 0.0], height);
                return;
            }
        };
        self.moved(camera);
    }

    /// The left button's events this frame that the pane doesn't take itself, for the tab of
    /// session `session`, in physical pixels from `rect`'s top left (the viewport's, as its
    /// frame gives it, not the canvas response's, which is rounded), of frame `frame`: a
    /// press on the canvas (where the ViewCube and the bar, on top, aren't, and not while
    /// the pane drags: the middle button's pan or orbit, or the cube's), the moves while it's
    /// held, its release or its cancel, and a double-click straight after the release that
    /// made it; none while a navigation tool makes a left drag navigate. `held` is the
    /// pane's, which [`end_unshown`] has already ended if another tab's. Each is placed by
    /// the egui event that made it ([`Placed`]), its `seq` given by [`sequenced`].
    fn left_events(
        &self,
        button: &mut LeftButton,
        session: SessionId,
        ui: &Ui,
        rect: Rect,
        response: &egui::Response,
        frame: u64,
    ) -> Placed<LeftEvent> {
        let ppp = ui.ctx().pixels_per_point();
        let px = |at: Pos2| [(at.x - rect.left()) * ppp, (at.y - rect.top()) * ppp];
        let options = ui.ctx().options(|options| options.input_options);
        // A navigation tool's drag: the pane's, which ends what was held. (The cube's can't
        // begin while the button is held from the canvas: egui gives a drag to the widget
        // pressed, and takes no other press until its release.)
        let taken = self.tool.is_some();
        let pressable = response.hovered() && self.drag.is_none() && !taken;
        let mut left = Vec::new();
        let LeftButton { held, clicks } = button;
        // Read where the events are, not cloned out each frame.
        ui.input(|input| {
            let time = input.time;
            let event = |place: usize, kind, at_px, keys: egui::Modifiers| {
                let event = LeftEvent {
                    kind,
                    at_px,
                    shift: keys.shift,
                    ctrl: keys.command,
                    frame,
                    time,
                    seq: 0,
                };
                (place, event)
            };
            if taken && let Some(was) = held.take() {
                left.push(event(0, LeftKind::Cancel, was.at_px, input.modifiers));
            }
            // Every left click is counted, the tool's and those off the canvas too, as egui
            // counts them; only a stream's are handed on.
            for (place, each) in (1..).zip(&input.events) {
                match *each {
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    } => {
                        clicks.pressed = Some(Pressed::new(pos, time));
                        if pressable && rect.contains(pos) {
                            // A press whose release never came ends before this one begins.
                            if let Some(was) = held.take() {
                                left.push(event(place, LeftKind::Cancel, was.at_px, modifiers));
                            }
                            let at_px = px(pos);
                            *held = Some(LeftHeld { session, at_px });
                            left.push(event(place, LeftKind::Press, at_px, modifiers));
                        }
                    }
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers,
                    } => {
                        let double = clicks.released(pos, time, &options);
                        if held.take().is_some() {
                            left.push(event(place, LeftKind::Release, px(pos), modifiers));
                            if double {
                                let at = px(pos);
                                left.push(event(place, LeftKind::DoubleClick, at, modifiers));
                            }
                        }
                    }
                    egui::Event::PointerMoved(pos) => {
                        if let Some(pressed) = clicks.pressed.as_mut() {
                            pressed.moved(pos, &options);
                        }
                        if let Some(held) = held.as_mut() {
                            held.at_px = px(pos);
                            let at = held.at_px;
                            left.push(event(place, LeftKind::Move, at, input.modifiers));
                        }
                    }
                    // The pointer leaving the window keeps the drag (egui's drags, and
                    // SolidWorks's past the window's edge): its release ends it there.
                    egui::Event::WindowFocused(false) => {
                        if let Some(was) = held.take() {
                            let at = was.at_px;
                            left.push(event(place, LeftKind::Cancel, at, input.modifiers));
                        }
                    }
                    _ => {}
                }
            }
        });
        left
    }

    /// The right button's clicks this frame on the canvas, for the tab of session
    /// `session`, in physical pixels from `rect`'s top left, of frame `frame`
    /// ([`SecondaryClick`]): each from a press on the canvas (where the ViewCube and the
    /// bar, on top, aren't, and not while the pane drags), at its release. `held` is the
    /// pane's: a press in another tab is no click in this one. Each is placed by the egui
    /// event that made it, as [`left_events`](Self::left_events)'s are.
    fn secondary_clicks(
        &self,
        held: &mut Option<SecondaryHeld>,
        session: SessionId,
        ui: &Ui,
        rect: Rect,
        response: &egui::Response,
        frame: u64,
    ) -> Placed<SecondaryClick> {
        let ppp = ui.ctx().pixels_per_point();
        let px = |at: Pos2| [(at.x - rect.left()) * ppp, (at.y - rect.top()) * ppp];
        let options = ui.ctx().options(|options| options.input_options);
        let pressable = response.hovered() && self.drag.is_none();
        ui.input(|input| {
            let time = input.time;
            let mut clicks = Vec::new();
            for (place, each) in (1..).zip(&input.events) {
                match *each {
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Secondary,
                        pressed: true,
                        ..
                    } => {
                        *held = (pressable && rect.contains(pos)).then(|| SecondaryHeld {
                            session,
                            pressed: Pressed::new(pos, time),
                        });
                    }
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Secondary,
                        pressed: false,
                        modifiers,
                    } => {
                        if let Some(was) = held.take()
                            && was.session == session
                            && was.pressed.clicked(pos, time, &options)
                        {
                            let click = SecondaryClick {
                                at_px: px(pos),
                                shift: modifiers.shift,
                                ctrl: modifiers.command,
                                frame,
                                time,
                                seq: 0,
                            };
                            clicks.push((place, click));
                        }
                    }
                    // Another button pressed or let go between: no click.
                    egui::Event::PointerButton { .. } => *held = None,
                    // The focus gone: its release may never come.
                    egui::Event::WindowFocused(false) => *held = None,
                    egui::Event::PointerMoved(pos) => {
                        if let Some(held) = held.as_mut() {
                            held.pressed.moved(pos, &options);
                        }
                    }
                    _ => {}
                }
            }
            clicks
        })
    }

    /// The person's navigation this frame (section 8, *Navigation*, Fusion's preset): the
    /// wheel zooms toward the cursor; the middle button pans, and orbits with Shift; a left
    /// drag does the navigation bar's tool, if one is chosen; a double-click of the middle
    /// button fits; and a view a shortcut asked for ([`ask`](Self::ask): F6's fit, Home's,
    /// or a named view's) is given. A drag ends when its button is let go, the pointer leaves the
    /// window, or the window loses the focus (whose release may never come).
    fn navigate(&mut self, ui: &mut Ui, response: &egui::Response, size_px: [u32; 2]) {
        let rect = response.rect;
        let height = f64::from(rect.height());
        let (pressed, origin, latest, shift, down, lost) = ui.input(|input| {
            let pointer = &input.pointer;
            let pressed = if pointer.button_pressed(PointerButton::Middle) {
                Some(PointerButton::Middle)
            } else if pointer.button_pressed(PointerButton::Primary) {
                Some(PointerButton::Primary)
            } else {
                None
            };
            let lost = input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::PointerGone | egui::Event::WindowFocused(false)
                )
            });
            (
                pressed,
                pointer.press_origin(),
                pointer.latest_pos(),
                input.modifiers.shift,
                [
                    pointer.button_down(PointerButton::Middle),
                    pointer.button_down(PointerButton::Primary),
                ],
                lost,
            )
        });
        if self.drag.is_none()
            && response.hovered()
            && let (Some(button), Some(origin)) = (pressed, origin)
            && (button == PointerButton::Middle || self.tool.is_some())
        {
            self.drag = Some(Drag::Canvas {
                button,
                last: origin,
            });
        }
        if let Some(Drag::Canvas { button, last }) = self.drag {
            let navigation = match button {
                PointerButton::Middle if shift => Some(Navigation::Orbit),
                PointerButton::Middle => Some(Navigation::Pan),
                _ => self.tool,
            };
            let at = latest.unwrap_or(last);
            if let Some(navigation) = navigation
                && at != last
            {
                self.navigated(navigation, at - last, height);
            }
            let held = match button {
                PointerButton::Middle => down[0],
                _ => down[1],
            };
            self.drag = (held && !lost).then_some(Drag::Canvas { button, last: at });
        }
        if response.hovered() {
            let line = ui
                .ctx()
                .options(|options| options.input_options.line_scroll_speed);
            let scroll: f32 = ui.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                            egui::MouseWheelUnit::Point => delta.y,
                            egui::MouseWheelUnit::Line => delta.y * line,
                            egui::MouseWheelUnit::Page => delta.y * rect.height(),
                        }),
                        _ => None,
                    })
                    .sum()
            });
            if scroll != 0.0
                && let Some(pointer) = latest
            {
                let cursor = pointer - rect.center();
                self.zoom(
                    f64::from(scroll),
                    [f64::from(cursor.x), f64::from(cursor.y)],
                    height,
                );
            }
        }
        match self.asked.take() {
            Some(ViewAsk::Named(named)) => self.look_from(named, size_px),
            Some(ViewAsk::Fit) => self.fit(size_px),
            None if response.double_clicked_by(PointerButton::Middle) => self.fit(size_px),
            None => {}
        }
    }
}

/// A frame's button events, each with its place: 0 for a Cancel a navigation tool forces
/// before the frame's events, `i + 1` for one the frame's `i`th egui event made.
type Placed<T> = Vec<(usize, T)>;

/// The frame's left events and right clicks, each given its `seq`: their order together,
/// by place, from 0. Two of one place are of one list (a Cancel and the Press that forced
/// it, a Release and its DoubleClick: one button's) and keep its order.
fn sequenced(
    left: Placed<LeftEvent>,
    secondary: Placed<SecondaryClick>,
) -> (Vec<LeftEvent>, Vec<SecondaryClick>) {
    let mut lefts = left.into_iter().peekable();
    let mut rights = secondary.into_iter().peekable();
    let (mut left, mut secondary) = (Vec::new(), Vec::new());
    let mut seq: u32 = 0;
    loop {
        let right_first = match (lefts.peek(), rights.peek()) {
            (Some((l, _)), Some((r, _))) => r < l,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => break,
        };
        if right_first {
            if let Some((_, mut click)) = rights.next() {
                click.seq = seq;
                secondary.push(click);
            }
        } else if let Some((_, mut event)) = lefts.next() {
            event.seq = seq;
            left.push(event);
        }
        seq = seq.saturating_add(1);
    }
    (left, secondary)
}

/// A pick asked: of the tab's session and its bodies table's generation, in the frame
/// numbered (the drawing's count) as its answer gives back.
type PickAsked = (SessionId, u64, u64);

/// What the renderer holds, for the tab shown: each body by number, with the session, the
/// key, and the deflection its mesh was uploaded for; each body whose upload it refused, as
/// it was asked; the picks asked and not yet answered; and how frames reach it.
#[derive(Default)]
pub struct Drawing {
    present: Option<Box<dyn Present>>,
    uploaded: BTreeMap<u32, (SessionId, Key, u64)>,
    refused: BTreeMap<u32, (SessionId, Key, u64)>,
    picks: VecDeque<PickAsked>,
    frames: u64,
    /// What says the time: the system's, or a test's.
    clock: Option<silico_core::disk::Clock>,
    /// Why there's no renderer, if one couldn't be made: the pane says so.
    unavailable: Option<String>,
    /// The left button: held from a press on the canvas, whichever tab it was pressed in,
    /// and its clicks.
    left: LeftButton,
    /// The right button held from a press on the canvas, whichever tab it was pressed in.
    secondary: Option<SecondaryHeld>,
}

impl Drawing {
    /// Frames drawn through `present`.
    pub fn new(present: Box<dyn Present>) -> Self {
        Drawing {
            present: Some(present),
            ..Drawing::default()
        }
    }

    /// No renderer, for `why`: frames go nowhere, and the pane says why.
    pub fn unavailable(why: String) -> Self {
        Drawing {
            unavailable: Some(why),
            ..Drawing::default()
        }
    }

    /// Frames drawn through `present`, `clock` saying the time (a test's, to move it by hand).
    pub fn clocked(present: Box<dyn Present>, clock: silico_core::disk::Clock) -> Self {
        Drawing {
            clock: Some(clock),
            ..Drawing::new(present)
        }
    }

    fn now(&self) -> Instant {
        self.clock
            .as_ref()
            .map_or_else(Instant::now, |clock| clock())
    }

    /// Whether body `number` is held by the renderer, to be drawn.
    pub fn holds(&self, number: u32) -> bool {
        self.uploaded.contains_key(&number)
    }

    /// The bodies the renderer refused since the last frame: not drawn, and not uploaded
    /// again until their mesh changes (a refusal for want of memory would be again).
    fn take_refused(&mut self) {
        let refused = match &mut self.present {
            Some(present) => present.refused(),
            None => return,
        };
        for number in refused {
            if let Some(asked) = self.uploaded.remove(&number) {
                self.refused.insert(number, asked);
            }
        }
    }

    /// Whether the renderer refused the last frame backing off ([`Present::backing_off`]).
    fn backing_off(&mut self) -> bool {
        self.present
            .as_mut()
            .is_some_and(|present| present.backing_off())
    }

    /// The changes that make the renderer hold `bodies` of session `session`'s tab (none for
    /// no tab): a body uploaded when its key or its deflection differs from what was
    /// uploaded for it, or it's another tab's; a body not among them forgotten. So each body
    /// is uploaded once for each mesh it's drawn with, whatever the frames between.
    pub fn changes(
        &mut self,
        session: Option<SessionId>,
        bodies: &BTreeMap<u32, ViewBody>,
    ) -> Vec<Change> {
        let mut changes = Vec::new();
        // A body refused is forgotten too when it goes: the renderer may hold an older mesh
        // of it.
        let gone: BTreeSet<u32> = self
            .uploaded
            .keys()
            .chain(self.refused.keys())
            .filter(|number| session.is_none() || !bodies.contains_key(number))
            .copied()
            .collect();
        for number in gone {
            self.uploaded.remove(&number);
            self.refused.remove(&number);
            changes.push(Change::Forget(number));
        }
        if let Some(session) = session {
            for (number, body) in bodies {
                let wanted = (session, body.key, body.deflection.to_bits());
                if self.refused.get(number) == Some(&wanted) {
                    continue;
                }
                self.refused.remove(number);
                if self.uploaded.get(number) != Some(&wanted) {
                    self.uploaded.insert(*number, wanted);
                    changes.push(Change::Upload(*number, Arc::clone(&body.mesh)));
                }
            }
        }
        changes
    }

    /// A pick asked in the frame about to be presented, of session `session`'s bodies table
    /// `generation`: the frame's number, which its answer gives back.
    fn asked(&mut self, session: SessionId, generation: u64) -> u64 {
        self.picks.push_back((session, generation, self.frames));
        self.frames
    }

    /// Whether a pick waits for its answer: the pane asks for frames until it comes.
    pub fn picking(&self) -> bool {
        !self.picks.is_empty()
    }

    /// The renderer's newest picking answer, and what it was asked of: the pick of the frame
    /// it names, exactly (answers come two frames late, so the newest pick of its rectangle
    /// may be another table's); those before it dropped. None for an answer to no pick
    /// waiting. A pick waits [`PICK_FRAMES`] frames after its own, and is let go after.
    fn answered(&mut self) -> Option<(Picking, SessionId, u64)> {
        let frames = self.frames;
        self.picks
            .retain(|(_, _, asked)| frames.saturating_sub(*asked) <= PICK_FRAMES);
        let picking = self.present.as_mut().and_then(|present| present.picked())?;
        let at = self
            .picks
            .iter()
            .position(|(_, _, serial)| *serial == picking.frame)?;
        let (session, generation, _) = self.picks.drain(..=at).next_back()?;
        Some((picking, session, generation))
    }

    /// A frame with no view in it (the start page, no document open): every body the
    /// renderer holds forgotten, and nothing drawn, as the pane does with no tab.
    pub(crate) fn nothing_shown(&mut self, painter: &egui::Painter) {
        let changes = self.changes(None, &BTreeMap::new());
        self.present(painter, Rect::NOTHING, changes, None);
    }

    fn present(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        changes: Vec<Change>,
        frame: Option<FrameData>,
    ) {
        self.frames = self.frames.wrapping_add(1);
        if let Some(present) = &mut self.present {
            present.present(painter, rect, changes, frame);
        }
    }
}

/// The box of a mesh's positions and edge points, if it has any.
fn extent(mesh: &Mesh) -> Option<[[f64; 3]; 2]> {
    silico_core::views::joined(
        mesh.positions
            .iter()
            .chain(&mesh.edge_points)
            .map(|point| [*point, *point]),
    )
}

/// A side in physical pixels, from a length in them.
fn pixels(length: f32) -> u32 {
    // A pane's side is a few thousand pixels at most; the cast saturates past u32 anyway.
    length.round().max(0.0) as u32
}

/// The viewport pane: the tab's view drawn, its navigation taken, its read started when
/// one is wanted, and its frame given to the renderer.
pub fn show<P: Clients + 'static>(
    ui: &mut Ui,
    tab: Option<&mut Tab>,
    sessions: &Arc<Sessions<P>>,
    drawing: &mut Drawing,
) {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    if let Some(why) = &drawing.unavailable {
        // Said where the model would be: nothing else in the window would say it.
        let said = format!("The viewport can't draw: {why}");
        let at = Rect::from_center_size(rect.center(), Vec2::new(rect.width().min(480.0), 60.0));
        ui.put(
            at,
            egui::Label::new(egui::RichText::new(said).color(ui.visuals().warn_fg_color)).wrap(),
        );
    }
    let ppp = ui.ctx().pixels_per_point();
    let size_px = [pixels(rect.width() * ppp), pixels(rect.height() * ppp)];
    let Some(tab) = tab else {
        let changes = drawing.changes(None, &BTreeMap::new());
        drawing.present(ui.painter(), rect, changes, None);
        return;
    };
    let now = drawing.now();
    let now_hidden: BTreeSet<FeatureId> = tab
        .browser
        .bodies
        .iter()
        .filter(|row| !tab.shown(&crate::tab::Hideable::Feature(row.id.clone())))
        .map(|row| row.id.clone())
        .collect();
    let view = &mut tab.view;
    if now_hidden != view.hidden {
        view.hidden = now_hidden;
        // A body shown again may reach farther than the view's least allowed.
        view.camera = navigate::clamped(view.camera.clone(), view.rows, view.drawn_boxes());
    }
    if size_px[1].max(1) != view.rows {
        // A taller pane's least is more: the view raised to it.
        view.rows = size_px[1].max(1);
        view.camera = navigate::clamped(view.camera.clone(), view.rows, view.drawn_boxes());
    }
    view.poll(now);
    match view.notice.take() {
        Some(Notice::Refused(said)) => tab.message = Some(said),
        Some(Notice::Answered(said)) if tab.message.as_deref() == Some(said.as_str()) => {
            tab.message = None;
        }
        _ => {}
    }
    // An answer to a pick of another tab, or of a table since made again, isn't this one's.
    if let Some((picking, session, generation)) = drawing.answered()
        && session == tab.id
        && generation == view.generation
    {
        view.picked = Some(picking);
    }
    drawing.take_refused();
    if drawing.backing_off() {
        // The renderer counts its backoff in frames: asked for at once, it lasts a second
        // at most, not 64 idle frames.
        ui.ctx().request_repaint();
    }
    view.navigate(ui, &response, size_px);
    let left = view.left_events(
        &mut drawing.left,
        tab.id,
        ui,
        rect,
        &response,
        drawing.frames,
    );
    let secondary = view.secondary_clicks(
        &mut drawing.secondary,
        tab.id,
        ui,
        rect,
        &response,
        drawing.frames,
    );
    let (left, secondary) = sequenced(left, secondary);
    let size = read_size(size_px);
    let settled = view.drag.is_none() && view.last.as_ref() == Some(&(view.camera.clone(), size));
    if !settled {
        // A frame after this one, to see the view settled.
        ui.ctx().request_repaint();
    }
    view.published_is(tab.published_models);
    match view.wanted(tab.published, size, settled, now) {
        Some(asked) => view.start(sessions, &tab.id, asked),
        // A retry due with no read wanted of the view at rest ends its run: a later refusal
        // starts again at the shortest delay. Not while it moves, which reads nothing.
        None => view.ended_if_due(settled, now),
    }
    if view.reading.is_none()
        && let Some(wait) = view.wake_in(now)
    {
        // A frame when the read refused for the moment may be asked again; none once due.
        ui.ctx().request_repaint_after(wait);
    }
    view.last = Some((view.camera.clone(), size));
    let changes = drawing.changes(Some(tab.id), &view.bodies);
    let pointer_px = ui
        .input(|input| input.pointer.latest_pos())
        .filter(|at| rect.contains(*at))
        .map(|at| [(at.x - rect.left()) * ppp, (at.y - rect.top()) * ppp]);
    view.frame = Some(ViewFrame {
        session: tab.id,
        rect,
        camera: view.camera.clone(),
        size_px,
        pixels_per_point: ppp,
        pointer_px,
        left,
        secondary,
    });
    // Ascending, each once, as the renderer takes them: the table's order.
    let bodies: Vec<u32> = view
        .bodies
        .iter()
        .filter(|(number, body)| !view.hidden.contains(&body.feature) && drawing.holds(**number))
        .map(|(number, _)| *number)
        .collect();
    // A highlight of a body not drawn, or of an element its mesh hasn't, would have the
    // renderer refuse the whole frame.
    let linked = view.linked_faces();
    let highlights = view
        .highlights
        .iter()
        .copied()
        .chain(linked)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|(number, element)| {
            bodies.binary_search(number).is_ok()
                && view
                    .bodies
                    .get(number)
                    .is_some_and(|body| body.has(*element))
        })
        .collect();
    let picking = view.picking.take();
    let serial = match picking {
        Some(_) => drawing.asked(tab.id, view.generation),
        None => drawing.frames,
    };
    let frame = FrameData {
        serial,
        camera: view.camera.clone(),
        size_px,
        pixels_per_point: ppp,
        bodies,
        highlights,
        picking,
    };
    drawing.present(ui.painter(), rect, changes, Some(frame));
    if drawing.picking() {
        // An idle egui draws no frame, and its answer is taken only in one.
        ui.ctx().request_repaint();
    }
    // Over the bodies, under the ViewCube and the bar, in the pane's own layer: what the
    // tab's mode draws (a sketch, a selection's marks). It takes no input; the cube's and
    // the bar's, registered after, come first. A camera they change is drawn next frame.
    if let Some(mut frame) = view.frame.take() {
        overlaid(tab, sessions, &ui.painter_at(rect), &frame);
        // Handed on once: what reads the frame after reads no event again.
        frame.left.clear();
        frame.secondary.clear();
        tab.view.frame = Some(frame);
    }
    view_cube(ui, rect, &mut tab.view, size_px);
    navigation_bar(ui, rect, &mut tab.view, size_px);
}

/// The tab's overlay given `frame`, and a test's watch after it, if one is set.
fn overlaid<P: Clients + 'static>(
    tab: &mut Tab,
    sessions: &Arc<Sessions<P>>,
    painter: &egui::Painter,
    frame: &ViewFrame,
) {
    tab.overlay(sessions, painter, frame);
    if let Some(watch) = &mut tab.watch {
        watch(painter, frame);
    }
}

/// Ends the left button's stream of a tab that isn't the one about to be shown (`shown`):
/// an open shown over it, Ctrl+W, or another tab chosen, mid-drag. Called each frame
/// before the pane draws. The stream's end is given to that tab's overlay with the frame it
/// was last shown in and a painter that draws nothing (it isn't shown): the button's
/// release, if one came this frame (a stalled frame that both let go and changed the tab),
/// at where it came, else a Cancel, where the stream was last; to none if the tab has gone.
/// The moves of this frame before it aren't given. The tab shown is given nothing of it:
/// the moves and the release to come aren't of a press of its own.
pub fn end_unshown<P: Clients + 'static>(
    ctx: &egui::Context,
    sessions: &Arc<Sessions<P>>,
    drawing: &mut Drawing,
    shown: Option<SessionId>,
    tabs: &mut [Tab],
) {
    let Some(held) = drawing.left.held.filter(|held| Some(held.session) != shown) else {
        return;
    };
    drawing.left.held = None;
    let Some(tab) = tabs.iter_mut().find(|tab| tab.id == held.session) else {
        return;
    };
    // It was shown when it was pressed in, so it has a frame.
    let Some(mut frame) = tab.view.frame.clone() else {
        return;
    };
    let (time, modifiers, released) = ctx.input(|input| {
        let released = input.events.iter().find_map(|event| match *event {
            egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers,
            } => Some((pos, modifiers)),
            _ => None,
        });
        (input.time, input.modifiers, released)
    });
    let (kind, at_px, keys) = match released {
        Some((at, keys)) => {
            let ppp = frame.pixels_per_point;
            let at_px = [
                (at.x - frame.rect.left()) * ppp,
                (at.y - frame.rect.top()) * ppp,
            ];
            (LeftKind::Release, at_px, keys)
        }
        None => (LeftKind::Cancel, held.at_px, modifiers),
    };
    frame.pointer_px = None;
    frame.left = vec![LeftEvent {
        kind,
        at_px,
        shift: keys.shift,
        ctrl: keys.command,
        frame: drawing.frames,
        time,
        seq: 0,
    }];
    let mut painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), Rect::NOTHING);
    painter.set_invisible();
    overlaid(tab, sessions, &painter, &frame);
}

/// The ViewCube's faces: each named view's face, its outward normal.
const FACES: [(NamedView, [f64; 3]); 6] = [
    (NamedView::Front, [0.0, -1.0, 0.0]),
    (NamedView::Back, [0.0, 1.0, 0.0]),
    (NamedView::Left, [-1.0, 0.0, 0.0]),
    (NamedView::Right, [1.0, 0.0, 0.0]),
    (NamedView::Top, [0.0, 0.0, 1.0]),
    (NamedView::Bottom, [0.0, 0.0, -1.0]),
];

/// A named view's words, as the ViewCube labels it: `top_front_right` is "Top front right".
fn words(view: NamedView) -> String {
    let spaced = view.name().replace('_', " ");
    let mut chars = spaced.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A clickable part of the ViewCube, labelled for the accessibility tree.
fn cube_part(ui: &mut Ui, rect: Rect, label: &str) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(("viewcube", label)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    response.on_hover_text(label)
}

/// The ViewCube's menu (fusion.md, *ViewCube and Navigation Bar*: *Camera*): the
/// projection, which the views it gives keep.
fn projection_menu(ui: &mut Ui, view: &mut View) {
    for (word, wanted) in [("Orthographic", false), ("Perspective", true)] {
        if ui.radio(view.perspective == wanted, word).clicked() {
            view.project(wanted);
            ui.close();
        }
    }
}

/// Where the ViewCube is drawn in `viewport`.
pub fn view_cube_rect(viewport: Rect) -> Rect {
    Rect::from_min_size(
        egui::pos2(
            viewport.right() - theme::VIEW_CUBE_SIDE - theme::VIEW_CUBE_MARGIN,
            viewport.top() + theme::VIEW_CUBE_MARGIN + theme::VIEW_CUBE_HOME_HEIGHT,
        ),
        Vec2::splat(theme::VIEW_CUBE_SIDE),
    )
}

/// The ViewCube, at the canvas's top right (section 8; fusion.md, *ViewCube and Navigation
/// Bar*): the cube as the camera sees it; a face's click gives its orthographic view, a
/// corner's its isometric one, a left drag orbits, and Home gives the home view; its menu
/// switches the projection, which the views it gives keep.
fn view_cube(ui: &mut Ui, viewport: Rect, view: &mut View, size_px: [u32; 2]) {
    let side = theme::VIEW_CUBE_SIDE;
    let rect = view_cube_rect(viewport);
    let Ok(frame) = view.camera.frame() else {
        return;
    };
    ui.scope_builder(UiBuilder::new().id_salt("viewcube"), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Group);
            node.set_label("ViewCube");
        });
        let cube = ui.interact(rect, ui.id().with("cube"), Sense::click_and_drag());
        cube.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Orbit the view"));
        if cube.dragged_by(PointerButton::Primary) {
            // Not settled while it's held, even where the pointer pauses.
            view.drag = Some(Drag::Cube);
            let delta = cube.drag_delta();
            let camera = navigate::orbit(&view.camera, f64::from(delta.x), f64::from(delta.y));
            view.moved(camera);
        } else if view.drag == Some(Drag::Cube) {
            view.drag = None;
        }
        let mut parts = vec![cube];
        let scale = f64::from(side) * 0.28;
        let at = |v: [f64; 3]| {
            let x = silico_protocol::vec3::dot(v, frame.right) * scale;
            let y = -silico_protocol::vec3::dot(v, frame.up) * scale;
            let offset = Vec2::new(x as f32, y as f32);
            rect.center() + offset
        };
        let painter = ui.painter_at(rect);
        let fill = ui.visuals().widgets.inactive.bg_fill;
        let stroke = Stroke::new(1.0, ui.visuals().widgets.inactive.fg_stroke.color);
        let mut chosen = None;
        for (named, normal) in FACES {
            let facing = silico_protocol::vec3::dot(normal, frame.back);
            if facing <= 1e-9 {
                continue;
            }
            // The face's two directions across it, and its corners in turn about it.
            let k = normal.iter().position(|c| *c != 0.0).unwrap_or(0);
            let (a, b) = ((k + 1) % 3, (k + 2) % 3);
            let corner = |sa: f64, sb: f64| {
                let mut v = normal;
                v[a] = sa;
                v[b] = sb;
                at(v)
            };
            let points = vec![
                corner(1.0, 1.0),
                corner(1.0, -1.0),
                corner(-1.0, -1.0),
                corner(-1.0, 1.0),
            ];
            let shade = fill.gamma_multiply(0.6 + 0.4 * facing as f32);
            painter.add(egui::Shape::convex_polygon(points, shade, stroke));
            if facing > 0.25 {
                let label = words(named);
                let centre = at(normal);
                painter.text(
                    centre,
                    egui::Align2::CENTER_CENTER,
                    &label,
                    egui::FontId::proportional(10.0),
                    ui.visuals().text_color(),
                );
                let part = Rect::from_center_size(centre, Vec2::splat(side * 0.3));
                let response = cube_part(ui, part, &label);
                if response.clicked() {
                    chosen = Some(named);
                }
                parts.push(response);
            }
        }
        for named in silico_core::views::NAMED_VIEWS {
            let direction = named.direction();
            if FACES.iter().any(|(face, _)| *face == named)
                || silico_protocol::vec3::dot(direction, frame.back) <= 0.1
            {
                continue;
            }
            let corner = at(direction.map(|c| c * 3f64.sqrt()));
            painter.circle_filled(corner, 3.0, stroke.color);
            let response = cube_part(
                ui,
                Rect::from_center_size(corner, Vec2::splat(12.0)),
                &words(named),
            );
            if response.clicked() {
                chosen = Some(named);
            }
            parts.push(response);
        }
        let home = Rect::from_min_size(
            rect.left_top() - Vec2::new(0.0, theme::VIEW_CUBE_HOME_HEIGHT),
            Vec2::new(44.0, theme::VIEW_CUBE_HOME_HEIGHT - 2.0),
        );
        if ui.put(home, egui::Button::new("Home").small()).clicked() {
            chosen = Some(HOME);
        }
        // A right-click anywhere on the cube opens its menu.
        for part in &parts {
            part.context_menu(|ui| projection_menu(ui, view));
        }
        if let Some(named) = chosen {
            view.look_from(named, size_px);
        }
    });
}

/// The navigation bar, floating at the canvas's bottom centre (section 8): Orbit, Pan, and
/// Zoom, each the left drag's tool while chosen (a second click lets it go), and Fit. A
/// press anywhere on it, its padding too, is its own, never the canvas's.
fn navigation_bar(ui: &mut Ui, viewport: Rect, view: &mut View, size_px: [u32; 2]) {
    let size = Vec2::new(theme::NAVIGATION_BAR_WIDTH, theme::NAVIGATION_BAR_HEIGHT);
    let rect = Rect::from_center_size(
        egui::pos2(
            viewport.center().x,
            viewport.bottom() - size.y / 2.0 - theme::NAVIGATION_BAR_MARGIN,
        ),
        size,
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("navigation_bar")
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center))
            // Under its buttons, over the canvas: the frame's padding absorbs a press.
            .sense(Sense::click_and_drag()),
        |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Toolbar);
                node.set_label("Navigation bar");
            });
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .corner_radius(theme::SMALL_CORNER_RADIUS)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (tool, word) in [
                            (Navigation::Orbit, "Orbit"),
                            (Navigation::Pan, "Pan"),
                            (Navigation::Zoom, "Zoom"),
                        ] {
                            let on = view.tool == Some(tool);
                            if ui.selectable_label(on, word).clicked() {
                                view.tool = (!on).then_some(tool);
                            }
                        }
                        if ui.button("Fit").clicked() {
                            view.fit(size_px);
                        }
                    });
                });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From −y onto (20, 10, 5), `view_height` high.
    fn front(view_height: f64) -> Camera {
        Camera {
            projection: Projection::Orthographic { view_height },
            eye: [20.0, -100.0, 5.0],
            target: [20.0, 10.0, 5.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    fn key(byte: u8) -> Key {
        Key::parse(&format!("{byte:02x}").repeat(32)).unwrap()
    }

    fn body(level: f64, serves: [f64; 2], bounds: Option<[[f64; 3]; 2]>) -> ViewBody {
        ViewBody {
            feature: FeatureId::new("m3tq8v0z").unwrap(),
            key: key(1),
            level,
            deflection: level,
            bounds,
            serves,
            held: false,
            mesh: Arc::new(Mesh {
                positions: Vec::new(),
                normals: Vec::new(),
                triangles: Vec::new(),
                face_triangles: Vec::new(),
                valid: None,
                edge_points: Vec::new(),
                edge_point_counts: Vec::new(),
            }),
            faces: Vec::new(),
            edges: Vec::new(),
        }
    }

    const CUT: Option<[[f64; 3]; 2]> = Some([[0.0, 0.0, -1.0], [40.0, 20.0, 11.0]]);

    #[test]
    fn an_orthographic_half_pixel_is_the_views_height_over_twice_its_pixels_at_any_depth() {
        // 80 mm over 1,000 pixels: 0.04 mm, the design's example, box or no box.
        assert_eq!(half_pixel(&front(80.0), 1000, CUT), Some(0.04));
        assert_eq!(half_pixel(&front(80.0), 1000, None), Some(0.04));
    }

    #[test]
    fn an_orthographic_view_reads_again_only_once_its_half_pixel_leaves_serves() {
        // Level 2⁻⁵ serves [0.03125, 0.0625): at 0.04, 0.03125 exactly, and just under
        // 0.0625 it's served; at 0.0625, and under 0.03125, it isn't. Half pixels by hand:
        // view heights 80, 62.5, and 124.99 over 1,000 pixels, then 125 and 62.4.
        let held = [body(0.03125, [0.03125, 0.0625], CUT)];
        for (height, served) in [
            (80.0, true),
            (62.5, true),
            (124.99, true),
            (125.0, false),
            (62.4, false),
        ] {
            assert_eq!(
                outside_serves(&front(height), 1000, &held),
                !served,
                "{height} mm high"
            );
        }
    }

    #[test]
    fn a_perspective_view_takes_each_bodys_half_pixel_at_its_nearest_depth() {
        // The design's case (section 13, M5.5's `serves`): the bracket's box (0, 0, 0) to
        // (60, 40, 50) from (100, −100, 100) toward (30, 20, 25), 30°, 1,024 pixels high:
        // the nearest corner's depth is 117.497 mm, its half pixel 0.030745 by hand, served
        // by level 2⁻⁶'s [0.015625, 0.03125); the box's centre would give 0.041311, which
        // isn't.
        let camera = Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            eye: [100.0, -100.0, 100.0],
            target: [30.0, 20.0, 25.0],
            up: [0.0, 0.0, 1.0],
        };
        let bracket = Some([[0.0, 0.0, 0.0], [60.0, 40.0, 50.0]]);
        let half = half_pixel(&camera, 1024, bracket).unwrap();
        assert!((half - 0.030745).abs() < 5e-7, "{half}");
        let held = [body(0.015625, [0.015625, 0.03125], bracket)];
        assert!(!outside_serves(&camera, 1024, &held));
        let coarser = [body(0.03125, [0.03125, 0.0625], bracket)];
        assert!(outside_serves(&camera, 1024, &coarser));
    }

    #[test]
    fn a_body_with_no_box_is_read_again_by_the_views_half_pixel_or_never_in_perspective() {
        // `box: null`. Orthographic: its level is the view's, as every body's, so it leaves
        // [0.03125, 0.0625) when the view does. Perspective: at no depth its level is its
        // floor, serving [0, 2 · mesh_level(floor)), and its half pixel is 0 at no depth,
        // so no move of the view reads it again.
        let unmeasured = [body(0.03125, [0.03125, 0.0625], None)];
        assert!(!outside_serves(&front(80.0), 1000, &unmeasured));
        assert!(outside_serves(&front(40.0), 1000, &unmeasured));
        let at_floor = [body(0.0087750, [0.0, 0.015625], None)];
        for eye in [[100.0, -100.0, 100.0], [1e5, -1e5, 1e5], [30.0, 19.0, 25.0]] {
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 30.0,
                },
                eye,
                target: [30.0, 20.0, 25.0],
                up: [0.0, 0.0, 1.0],
            };
            assert_eq!(half_pixel(&camera, 1024, None), Some(0.0));
            assert!(!outside_serves(&camera, 1024, &at_floor), "{eye:?}");
        }
    }

    fn view_with(bodies: &[(u32, ViewBody)], model: u64) -> View {
        View {
            camera: front(80.0),
            placed: true,
            bodies: bodies.iter().cloned().collect(),
            model: Revision::from_number(model),
            rows: 400,
            ..View::default()
        }
    }

    fn now() -> Instant {
        // One instant for every call in a test: no retry waits on it.
        static NOW: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *NOW.get_or_init(Instant::now)
    }

    fn revision(n: u64) -> Option<Revision> {
        Revision::from_number(n)
    }

    #[test]
    fn a_read_is_wanted_for_a_new_published_revision_and_for_a_settled_view_outside_serves() {
        let held = [(1, body(0.03125, [0.03125, 0.0625], CUT))];
        let mut view = view_with(&held, 3);
        // Nothing published: nothing to read.
        assert_eq!(view.wanted(None, [1000, 1000], true, now()), None);
        // The bodies are of revision 3: none at 3, one at 4, settled or not.
        assert_eq!(view.wanted(revision(3), [1000, 1000], true, now()), None);
        assert!(
            view.wanted(revision(4), [1000, 1000], false, now())
                .is_some()
        );
        // Zoomed in to 40 mm: 0.02 leaves serves, but only a settled view reads.
        view.camera = front(40.0);
        assert_eq!(view.wanted(revision(3), [1000, 1000], false, now()), None);
        let asked = view.wanted(revision(3), [1000, 1000], true, now()).unwrap();
        assert_eq!(asked.camera, front(40.0));
        assert_eq!(asked.size, [1000, 1000]);
        // A narrower window changes nothing orthographic but its height: 2,000 pixels high
        // at 80 mm halves the half pixel, out of serves too.
        view.camera = front(80.0);
        assert_eq!(view.wanted(revision(3), [500, 1000], true, now()), None);
        assert!(
            view.wanted(revision(3), [1000, 2000], true, now())
                .is_some()
        );
    }

    #[test]
    fn a_model_published_again_at_the_same_revision_is_read_again_once() {
        let held = [(1, body(0.03125, [0.03125, 0.0625], CUT))];
        let mut view = view_with(&held, 3);
        let answered = || Response {
            session: None,
            model: None,
            result: Ok(MeshesRead {
                camera: AnsweredCamera::of(&front(80.0)).unwrap(),
                bodies: Vec::new(),
                coarser: "height",
            }),
        };
        assert_eq!(view.wanted(revision(3), [1000, 1000], true, now()), None);
        // Another model of revision 3 (a fresh check's): wanted, settled or not.
        view.published_is(1);
        let asked = view
            .wanted(revision(3), [1000, 1000], false, now())
            .unwrap();
        assert_eq!((asked.revision, asked.model), (revision(3).unwrap(), 1));
        // Answered: read, not wanted again.
        view.took(asked, false, answered(), now());
        assert_eq!(view.wanted(revision(3), [1000, 1000], true, now()), None);
        // A read asked of one model, answering after another was published: stale, and
        // read again.
        view.published_is(2);
        let asked = view.wanted(revision(3), [1000, 1000], true, now()).unwrap();
        view.published_is(3);
        view.took(asked, false, answered(), now());
        let again = view.wanted(revision(3), [1000, 1000], true, now()).unwrap();
        assert_eq!(again.model, 3);
    }

    #[test]
    fn a_refused_read_is_not_asked_again_until_the_view_differs_and_settles() {
        let held = [(1, body(0.03125, [0.03125, 0.0625], CUT))];
        let mut view = view_with(&held, 3);
        let asked = view.wanted(revision(4), [1000, 1000], true, now()).unwrap();
        view.refused = Some(asked);
        assert_eq!(view.wanted(revision(4), [1000, 1000], true, now()), None);
        // Moving, not settled: still none (a drag would read each frame).
        view.camera = front(81.0);
        assert_eq!(view.wanted(revision(4), [1000, 1000], false, now()), None);
        // Settled elsewhere: once more; and a newer revision at once.
        assert!(
            view.wanted(revision(4), [1000, 1000], true, now())
                .is_some()
        );
        assert!(
            view.wanted(revision(5), [1000, 1000], false, now())
                .is_some()
        );
    }

    fn meshed(number: u32, key_byte: u8, deflection: f64, mesh: Option<Arc<Mesh>>) -> MeshedBody {
        MeshedBody {
            feature: FeatureId::new("m3tq8v0z").unwrap(),
            body: Some(number),
            key: key(key_byte),
            level: deflection,
            deflection,
            bounds: CUT,
            serves: [deflection, 2.0 * deflection],
            held: mesh.is_none(),
            mesh,
            faces: Vec::new(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn a_held_body_keeps_the_mesh_the_tab_holds_of_its_key_at_its_deflection() {
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let mesh = Arc::clone(&view.bodies[&1].mesh);
        // Renumbered (a body before it, new): the same mesh, by its key and deflection.
        view.take_bodies(vec![meshed(2, 1, 0.03125, None)]);
        assert!(Arc::ptr_eq(&view.bodies[&2].mesh, &mesh));
        assert!(!view.bodies.contains_key(&1));
        assert!(!view.hold_none);
        // Held at another deflection, which the tab hasn't: left out, and the next read
        // holds nothing, and is wanted at once.
        view.take_bodies(vec![meshed(2, 1, 0.0625, None)]);
        assert!(view.bodies.is_empty());
        assert!(view.hold_none);
        assert_eq!(view.model, None);
    }

    #[test]
    fn the_renderer_is_told_of_each_body_once_for_each_mesh_and_of_a_body_gone() {
        let mut drawing = Drawing::default();
        let s1 = SessionId::parse("s1").unwrap();
        let s2 = SessionId::parse("s2").unwrap();
        let mut bodies: BTreeMap<u32, ViewBody> = [
            (1, body(0.03125, [0.03125, 0.0625], CUT)),
            (2, body(0.03125, [0.03125, 0.0625], CUT)),
        ]
        .into_iter()
        .collect();
        let told = |changes: Vec<Change>| -> Vec<String> {
            changes
                .into_iter()
                .map(|change| match change {
                    Change::Upload(n, _) => format!("upload {n}"),
                    Change::Forget(n) => format!("forget {n}"),
                })
                .collect()
        };
        assert_eq!(
            told(drawing.changes(Some(s1), &bodies)),
            ["upload 1", "upload 2"]
        );
        for _ in 0..100 {
            assert!(drawing.changes(Some(s1), &bodies).is_empty());
        }
        // Body 2 at another level: it alone, again.
        bodies.get_mut(&2).unwrap().deflection = 0.015625;
        assert_eq!(told(drawing.changes(Some(s1), &bodies)), ["upload 2"]);
        // At another key, the same level: again.
        bodies.get_mut(&1).unwrap().key = key(7);
        assert_eq!(told(drawing.changes(Some(s1), &bodies)), ["upload 1"]);
        // Body 1 gone.
        bodies.remove(&1);
        assert_eq!(told(drawing.changes(Some(s1), &bodies)), ["forget 1"]);
        // Another tab's body 2, the same key and level: its own, so uploaded.
        assert_eq!(told(drawing.changes(Some(s2), &bodies)), ["upload 2"]);
        // No tab: everything forgotten.
        assert_eq!(told(drawing.changes(None, &bodies)), ["forget 2"]);
    }

    /// A present that refuses the bodies it's told to.
    struct Refusing(BTreeSet<u32>);

    impl Present for Refusing {
        fn present(&mut self, _: &egui::Painter, _: Rect, _: Vec<Change>, _: Option<FrameData>) {}
        fn picked(&mut self) -> Option<Picking> {
            None
        }
        fn refused(&mut self) -> BTreeSet<u32> {
            std::mem::take(&mut self.0)
        }
    }

    #[test]
    fn a_body_whose_upload_was_refused_isnt_held_or_uploaded_again_until_its_mesh_changes() {
        let mut drawing = Drawing::new(Box::new(Refusing([2].into_iter().collect())));
        let s1 = SessionId::parse("s1").unwrap();
        let mut bodies: BTreeMap<u32, ViewBody> = [
            (1, body(0.03125, [0.03125, 0.0625], CUT)),
            (2, body(0.03125, [0.03125, 0.0625], CUT)),
        ]
        .into_iter()
        .collect();
        assert_eq!(drawing.changes(Some(s1), &bodies).len(), 2);
        drawing.take_refused();
        assert!(drawing.holds(1) && !drawing.holds(2));
        assert!(drawing.changes(Some(s1), &bodies).is_empty());
        // Its level changes: tried again.
        bodies.get_mut(&2).unwrap().deflection = 0.015625;
        let again = drawing.changes(Some(s1), &bodies);
        assert!(matches!(again[..], [Change::Upload(2, _)]));
        assert!(drawing.holds(2));
        // Refused again, then gone: forgotten, as the renderer may hold an older mesh.
        drawing.present = Some(Box::new(Refusing([2].into_iter().collect())));
        drawing.take_refused();
        bodies.remove(&2);
        let gone = drawing.changes(Some(s1), &bodies);
        assert!(matches!(gone[..], [Change::Forget(2)]));
    }

    #[test]
    fn a_read_asks_at_most_the_largest_side_a_read_takes() {
        assert_eq!(read_size([0, 5000]), [1, 4096]);
        assert_eq!(read_size([1600, 1000]), [1600, 1000]);
    }

    #[test]
    fn the_view_cubes_labels_are_the_named_views_in_words() {
        assert_eq!(words(NamedView::TopFrontRight), "Top front right");
        assert_eq!(words(NamedView::Front), "Front");
    }

    #[test]
    fn the_view_cube_sits_its_margin_in_from_the_canvass_top_right_under_home() {
        // A canvas from (100, 50) to (900, 650): the cube's 92-point square (the prototype's
        // `CUBE`) 16 points (`CUBE_M`) in from its right edge, 900 − 16 − 92 = 792 to
        // 900 − 16 = 884, and 16 points down past Home's 20-point row, 50 + 16 + 20 = 86 to
        // 86 + 92 = 178.
        let viewport = Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(900.0, 650.0));
        assert_eq!(
            view_cube_rect(viewport),
            Rect::from_min_max(egui::pos2(792.0, 86.0), egui::pos2(884.0, 178.0))
        );
    }

    #[test]
    fn a_view_at_a_levels_edge_by_its_rounding_reads_once() {
        // 1,000 pixels high, 0.24414062 mm: unrounded, its half pixel is 0.00012207031, just
        // under 2⁻¹³ (0.0001220703125); the core levels with the camera an answer writes, to
        // the nanometre, 0.244141 mm, whose half pixel 0.0001220705 is at 2⁻¹³, served by
        // [2⁻¹³, 2⁻¹²). Taken unrounded, it would leave serves at every settled frame.
        let level = 1.0 / 8192.0;
        let held = [(1, body(level, [level, 2.0 * level], CUT))];
        let mut view = view_with(&held, 3);
        view.camera = front(0.244_140_62);
        assert_eq!(view.wanted(revision(3), [1000, 1000], true, now()), None);
        assert!(!outside_serves(&view.camera, 1000, view.bodies.values()));
    }

    #[test]
    fn the_read_last_answered_isnt_asked_again_however_its_answer_reads() {
        // An answer whose serves don't hold the view's half pixel (0.04, [0.0625, 0.125)):
        // the same read, asked again, would answer the same.
        let mut view = view_with(&[(1, body(0.0625, [0.0625, 0.125], CUT))], 3);
        let asked = view.wanted(revision(3), [1000, 1000], true, now()).unwrap();
        view.answered = Some(asked);
        assert_eq!(view.wanted(revision(3), [1000, 1000], true, now()), None);
        // Another size is another read.
        assert!(view.wanted(revision(3), [1000, 999], true, now()).is_some());
    }

    fn refusal(code: ErrorCode, parameter: Option<&str>) -> Response<MeshesRead> {
        // Worded by what it names, as two refusals of different causes are.
        let mut error =
            silico_core::Error::new(code, format!("{} refused", parameter.unwrap_or("a read")));
        if let Some(parameter) = parameter {
            error = error.with_parameter(parameter);
        }
        Response {
            session: None,
            model: None,
            result: Err(Problems::from(error)),
        }
    }

    fn asked(view: &View, published: u64) -> Asked {
        Asked {
            revision: Revision::from_number(published).unwrap(),
            model: view.published,
            camera: view.camera.clone(),
            size: [1000, 1000],
        }
    }

    #[test]
    fn a_read_refused_for_the_moment_is_asked_again_after_a_delay_that_doubles_to_eight_seconds() {
        let at = now();
        for (code, parameter) in [
            (ErrorCode::WorkerCrashed, None),
            (ErrorCode::WorkerTimeout, None),
            (ErrorCode::Internal, None),
            // Too many published reads at once: `invalid_input`, naming no parameter.
            (ErrorCode::InvalidInput, None),
        ] {
            let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
            let mut wait = Duration::from_millis(250);
            for _ in 0..8 {
                let request = asked(&view, 4);
                view.took(request, false, refusal(code, parameter), at);
                assert_eq!(view.wanted(revision(4), [1000, 1000], true, at), None);
                let early = at + wait - Duration::from_millis(1);
                assert_eq!(view.wanted(revision(4), [1000, 1000], true, early), None);
                assert!(
                    view.wanted(revision(4), [1000, 1000], true, at + wait)
                        .is_some()
                );
                wait = (wait * 2).min(Duration::from_secs(8));
            }
            assert_eq!(wait, Duration::from_secs(8), "{code:?}");
            assert!(view.refused.is_none());
            assert!(matches!(view.notice, Some(Notice::Refused(_))));
        }
    }

    #[test]
    fn a_read_refused_for_what_it_asked_isnt_asked_again_until_the_view_differs() {
        // An eye inside a solid (`camera.eye`), a view, a size, features: the same read
        // would be refused again, so it isn't asked, however long it waits.
        for parameter in ["camera.eye", "view", "width", "height", "features.0"] {
            let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
            let request = asked(&view, 4);
            view.took(
                request,
                false,
                refusal(ErrorCode::InvalidInput, Some(parameter)),
                now(),
            );
            assert!(view.retry.is_none(), "{parameter}");
            let later = now() + Duration::from_secs(3600);
            assert_eq!(view.wanted(revision(4), [1000, 1000], true, later), None);
            view.camera = front(81.0);
            assert!(
                view.wanted(revision(4), [1000, 1000], true, later)
                    .is_some()
            );
        }
    }

    #[test]
    fn a_missing_held_mesh_makes_the_next_request_hold_nothing_once() {
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        view.take_bodies(vec![meshed(2, 1, 0.0625, None)]);
        view.take_bodies(vec![meshed(
            1,
            1,
            0.03125,
            Some(Arc::clone(&body(0.03125, [0.03125, 0.0625], CUT).mesh)),
        )]);
        let request = asked(&view, 3);
        assert!(view.request(&request).held.is_empty());
        let again = view.request(&request);
        assert_eq!(again.held.len(), 1);
        assert_eq!(again.held[0].key, view.bodies[&1].key);
        assert_eq!(again.held[0].key, key(1));
        assert_eq!(again.held[0].deflection, 0.03125);
        // Placed: of the camera, not Home's view.
        assert_eq!((again.view, again.camera.is_some()), (None, true));
        view.placed = false;
        let first = view.request(&request);
        assert_eq!((first.view, first.camera), (Some(HOME), None));
    }

    /// A present whose picking answers it's given.
    struct Answering(VecDeque<Picking>);

    impl Present for Answering {
        fn present(&mut self, _: &egui::Painter, _: Rect, _: Vec<Change>, _: Option<FrameData>) {}
        fn picked(&mut self) -> Option<Picking> {
            self.0.pop_front()
        }
        fn refused(&mut self) -> BTreeSet<u32> {
            BTreeSet::new()
        }
    }

    fn painter() -> egui::Painter {
        egui::Painter::new(
            egui::Context::default(),
            egui::LayerId::background(),
            Rect::EVERYTHING,
        )
    }

    fn answer(frame: u64) -> Picking {
        Picking {
            frame,
            rect: PixelRect {
                min: [4, 4],
                size: [3, 3],
            },
            hit: None,
        }
    }

    #[test]
    fn a_picking_answer_is_matched_to_the_frame_it_names_not_the_newest_pick_of_its_rectangle() {
        // One rectangle asked in frame 0 of table 1, and again in frame 1 of table 2 (the
        // bodies read again between): frame 0's answer, two frames late, is table 1's, though
        // the newest pick of its rectangle is table 2's. Then frame 1's answer, table 2's.
        let s1 = SessionId::parse("s1").unwrap();
        let mut drawing = Drawing::new(Box::new(Answering(
            [answer(0), answer(1)].into_iter().collect(),
        )));
        assert_eq!(drawing.asked(s1, 1), 0);
        drawing.present(&painter(), Rect::EVERYTHING, Vec::new(), None);
        assert_eq!(drawing.asked(s1, 2), 1);
        drawing.present(&painter(), Rect::EVERYTHING, Vec::new(), None);
        assert_eq!(drawing.answered(), Some((answer(0), s1, 1)));
        assert_eq!(drawing.answered(), Some((answer(1), s1, 2)));
        assert!(!drawing.picking());
    }

    #[test]
    fn a_pick_waits_eight_frames_after_its_own_for_its_answer_and_no_more() {
        let s1 = SessionId::parse("s1").unwrap();
        for (frames, kept) in [(8, true), (9, false)] {
            let mut drawing = Drawing::new(Box::new(Answering([answer(0)].into_iter().collect())));
            assert_eq!(drawing.asked(s1, 1), 0);
            for _ in 0..frames {
                drawing.present(&painter(), Rect::EVERYTHING, Vec::new(), None);
            }
            assert_eq!(drawing.answered().is_some(), kept, "{frames} frames");
            assert!(!drawing.picking());
        }
    }

    #[test]
    fn a_due_retry_no_read_is_wanted_for_asks_for_no_more_frames() {
        // Refused for the moment at t: a frame is asked for in 250 ms, and none at t + 250
        // ms once the view is back where the last answer was (no read wanted).
        let at = now();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let settled = asked(&view, 3);
        view.answered = Some(settled);
        view.camera = front(40.0);
        let request = asked(&view, 3);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), at);
        assert_eq!(view.wake_in(at), Some(Duration::from_millis(250)));
        assert_eq!(
            view.wake_in(at + Duration::from_millis(100)),
            Some(Duration::from_millis(150))
        );
        view.camera = front(80.0);
        let due = at + Duration::from_millis(250);
        assert_eq!(view.wanted(revision(3), [1000, 1000], true, due), None);
        assert_eq!(view.wake_in(due), None);
        assert_eq!(view.wake_in(due + Duration::from_secs(1)), None);
        // A refusal of what was asked lets the retry go.
        view.camera = front(40.0);
        let request = asked(&view, 3);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), due);
        assert!(view.retry.is_some());
        let request = asked(&view, 3);
        view.took(
            request,
            false,
            refusal(ErrorCode::InvalidInput, Some("camera.eye")),
            due,
        );
        assert!(view.retry.is_none());
    }

    #[test]
    fn a_backoff_holds_back_every_read_of_its_revision_till_its_time_never_a_new_revision() {
        // Seven refusals in a row: the next asked in 8 s. A new revision is read a
        // millisecond later; the view moved and settled is not, until the 8 s are up, and
        // then the view as it is then.
        let at = now();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        for _ in 0..7 {
            let request = asked(&view, 4);
            view.took(request, false, refusal(ErrorCode::WorkerTimeout, None), at);
        }
        assert_eq!(view.wake_in(at), Some(Duration::from_secs(8)));
        let soon = at + Duration::from_millis(1);
        assert_eq!(view.wanted(revision(4), [1000, 1000], true, soon), None);
        assert!(
            view.wanted(revision(5), [1000, 1000], false, soon)
                .is_some()
        );
        view.camera = front(40.0);
        assert_eq!(view.wanted(revision(4), [1000, 1000], true, soon), None);
        let due = at + Duration::from_secs(8);
        let asked = view.wanted(revision(4), [1000, 1000], true, due).unwrap();
        assert_eq!(asked.camera, front(40.0));
    }

    #[test]
    fn a_refusal_is_said_once_a_revision_not_at_every_retry() {
        let at = now();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let request = asked(&view, 4);
        view.took(request, false, refusal(ErrorCode::Internal, None), at);
        assert!(matches!(view.notice.take(), Some(Notice::Refused(_))));
        for _ in 0..5 {
            let request = asked(&view, 4);
            view.took(request, false, refusal(ErrorCode::Internal, None), at);
            assert!(view.notice.is_none());
        }
        // The next revision's refusal is said.
        let request = asked(&view, 5);
        view.took(request, false, refusal(ErrorCode::Internal, None), at);
        assert!(matches!(view.notice.take(), Some(Notice::Refused(_))));
    }

    #[test]
    fn a_view_under_the_least_a_new_table_allows_is_raised_to_it() {
        // Zoomed to 1e-5 mm onto (20, 10, 5) before any body came, in a pane 400 pixels
        // high: the cut's box (0, 0, −1) to (40, 20, 11) reaches √(20² + 10² + 6²) = 23.152 mm
        // from there, so the least is 400 · 23.152 / 2^19 · 1.01 = 0.017838 mm, which the
        // view is raised to.
        let mut view = View {
            rows: 400,
            ..View::default()
        };
        view.set_camera(front(1e-5));
        view.take_bodies(vec![meshed(
            1,
            1,
            0.03125,
            Some(Arc::clone(&body(0.03125, [0.03125, 0.0625], CUT).mesh)),
        )]);
        let least = 536f64.sqrt() * 400.0 / 524_288.0 * 1.01;
        let Projection::Orthographic { view_height } = view.camera.projection else {
            panic!("orthographic")
        };
        assert!((view_height - least).abs() < 1e-18, "{view_height}");
        // From there, a zoom in stays, wherever the cursor is: the point under it, and so
        // the target, unmoved; out, it doubles.
        let target = view.camera.target;
        view.zoom(200.0, [100.0, -50.0], 400.0);
        assert_eq!(
            view.camera.projection,
            Projection::Orthographic { view_height }
        );
        assert_eq!(view.camera.target, target);
        view.zoom(-200.0, [0.0, 0.0], 400.0);
        assert_eq!(
            view.camera.projection,
            Projection::Orthographic {
                view_height: view_height * 2.0
            }
        );
    }

    #[test]
    fn a_highlight_is_of_an_element_the_bodys_mesh_has() {
        let mut held = body(0.03125, [0.03125, 0.0625], CUT);
        held.mesh = Arc::new(Mesh {
            face_triangles: vec![1, 1],
            edge_point_counts: vec![2, 2, 2],
            ..(*held.mesh).clone()
        });
        assert!(held.has(Element::Face(1)) && held.has(Element::Face(2)));
        assert!(!held.has(Element::Face(0)) && !held.has(Element::Face(3)));
        assert!(held.has(Element::Edge(3)) && !held.has(Element::Edge(4)));
    }

    #[test]
    fn every_camera_a_control_makes_is_raised_to_the_least_its_target_allows() {
        // A view 1e-6 mm high, its target 1,000 mm from the cut: the box's farthest corner
        // from (1020, 10, 5) is (0, 0, −1)'s, √(1020² + 10² + 6²) away.
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let far = Camera {
            projection: Projection::Orthographic { view_height: 1e-6 },
            eye: [1020.0, -100.0, 5.0],
            target: [1020.0, 10.0, 5.0],
            up: [0.0, 0.0, 1.0],
        };
        assert!(view.moved(Some(far)));
        let least = (1020.0f64 * 1020.0 + 100.0 + 36.0).sqrt() * 400.0 / 524_288.0 * 1.01;
        let Projection::Orthographic { view_height } = view.camera.projection else {
            panic!("orthographic")
        };
        assert!((view_height - least).abs() < 1e-15, "{view_height}");
        assert!(!view.moved(None));
    }

    #[test]
    fn a_projection_the_camera_cant_take_isnt_the_views() {
        // 1e12 mm high: in perspective at 30° its eye would be 1.87e12 mm back, past what a
        // camera may be, so the view stays orthographic.
        let mut view = view_with(&[], 3);
        view.camera = front(1e12);
        view.project(true);
        assert!(!view.perspective);
        assert_eq!(view.camera, front(1e12));
        view.camera = front(80.0);
        view.project(true);
        assert!(view.perspective);
    }

    #[test]
    fn a_retry_waiting_holds_back_its_revisions_reads_moving_or_settled() {
        // Refused for the moment at revision 4 (the bodies are of 3, so every view of 4
        // would read): while the view moves, and once it settles elsewhere, no read of 4
        // until 250 ms; then one, of the view then; refused again, the delay grows (500 ms).
        let at = now();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let request = asked(&view, 4);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), at);
        for height in [81.0, 82.0, 83.0] {
            view.camera = front(height);
            assert_eq!(view.wanted(revision(4), [1000, 1000], false, at), None);
            assert_eq!(view.wanted(revision(4), [1000, 1000], true, at), None);
        }
        let due = at + Duration::from_millis(250);
        let elsewhere = view.wanted(revision(4), [1000, 1000], true, due).unwrap();
        assert_eq!(elsewhere.camera, front(83.0));
        view.took(
            elsewhere,
            false,
            refusal(ErrorCode::WorkerCrashed, None),
            due,
        );
        assert_eq!(view.wake_in(due), Some(Duration::from_millis(500)));
        // A new revision, moving or not, at once.
        assert!(view.wanted(revision(5), [1000, 1000], false, due).is_some());
    }

    #[test]
    fn two_refusals_of_one_revision_are_each_said_and_an_answer_forgets_them() {
        let at = now();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let request = asked(&view, 4);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), at);
        assert!(matches!(view.notice.take(), Some(Notice::Refused(_))));
        let request = asked(&view, 4);
        view.took(
            request,
            false,
            refusal(ErrorCode::InvalidInput, Some("camera.eye")),
            at,
        );
        assert!(matches!(view.notice.take(), Some(Notice::Refused(_))));
        // An answer: what was said is let go, so the same refusal again is said again.
        let request = asked(&view, 4);
        view.took(
            request,
            false,
            Response {
                session: None,
                model: None,
                result: Ok(MeshesRead {
                    camera: AnsweredCamera::of(&front(80.0)).unwrap(),
                    bodies: Vec::new(),
                    coarser: "height",
                }),
            },
            at,
        );
        assert!(matches!(view.notice.take(), Some(Notice::Answered(_))));
        assert_eq!(view.said_for, None);
        let request = asked(&view, 4);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), at);
        assert!(matches!(view.notice.take(), Some(Notice::Refused(_))));
    }

    #[test]
    fn a_read_answered_of_a_newer_revision_than_asked_is_not_asked_again() {
        // A published read takes the newest model as its thread starts, not as it's sent:
        // asked at revision 4 (the bodies of 3), it may answer of 5, published meanwhile.
        // The bodies are then 5's, and 5 is read no more; 6 is. One answered of 4, as asked,
        // with 5 published, leaves 5 to read.
        let answer = |revision: u64| Response {
            session: None,
            model: Some(silico_core::command::ModelRead {
                revision: Revision::from_number(revision).unwrap(),
                of: ModelChoice::Published,
                fault: None,
            }),
            result: Ok(MeshesRead {
                camera: AnsweredCamera::of(&front(80.0)).unwrap(),
                bodies: vec![meshed(1, 1, 0.03125, None)],
                coarser: "height",
            }),
        };
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let mesh = Arc::clone(&view.bodies[&1].mesh);
        let request = asked(&view, 4);
        view.took(request, false, answer(5), now());
        assert_eq!(view.model(), revision(5));
        assert!(Arc::ptr_eq(&view.bodies[&1].mesh, &mesh));
        for settled in [false, true] {
            assert_eq!(view.wanted(revision(5), [1000, 1000], settled, now()), None);
        }
        assert!(
            view.wanted(revision(6), [1000, 1000], false, now())
                .is_some()
        );
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let request = asked(&view, 4);
        view.took(request, false, answer(4), now());
        assert_eq!(view.model(), revision(4));
        let again = view
            .wanted(revision(5), [1000, 1000], false, now())
            .unwrap();
        assert_eq!(again.revision, Revision::from_number(5).unwrap());
    }

    #[test]
    fn an_answer_names_its_own_frame_when_an_earlier_answer_never_came() {
        // Frames 0 and 1 picked; only frame 1's answer comes: it's frame 1's pick (table
        // 2), frame 0's let go with it; then an answer naming no waiting pick gives none.
        let s1 = SessionId::parse("s1").unwrap();
        let mut drawing = Drawing::new(Box::new(Answering(
            [answer(1), answer(0)].into_iter().collect(),
        )));
        assert_eq!(drawing.asked(s1, 1), 0);
        drawing.present(&painter(), Rect::EVERYTHING, Vec::new(), None);
        assert_eq!(drawing.asked(s1, 2), 1);
        drawing.present(&painter(), Rect::EVERYTHING, Vec::new(), None);
        assert_eq!(drawing.answered(), Some((answer(1), s1, 2)));
        assert!(!drawing.picking());
        // Frame 2 picked; frame 0's answer comes at last: it names no pick waiting, so it's
        // none, and frame 2's pick waits on.
        drawing.present(&painter(), Rect::EVERYTHING, Vec::new(), None);
        assert_eq!(drawing.asked(s1, 3), 3);
        assert_eq!(drawing.answered(), None);
        assert!(drawing.picking());
    }

    #[test]
    fn the_least_a_view_is_clamped_to_is_by_the_bodies_drawn_and_an_unmeasured_ones_mesh() {
        // The cut, and a body 10 m off: hidden, it's no part of the reach from (20, 10, 5),
        // √536, as the renderer, drawing it not, leaves it out. A body with no box, whose
        // mesh reaches (200, 10, 5) by an edge's point, counts by that: 180 mm.
        let far = ViewBody {
            feature: FeatureId::new("a91cw0de").unwrap(),
            ..body(0.03125, [0.03125, 0.0625], Some([[1e4; 3], [1.1e4; 3]]))
        };
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT)), (2, far)], 3);
        view.hidden = [FeatureId::new("a91cw0de").unwrap()].into_iter().collect();
        let tiny = Camera {
            projection: Projection::Orthographic { view_height: 1e-6 },
            ..front(80.0)
        };
        let height = |view: &View| match view.camera.projection {
            Projection::Orthographic { view_height } => view_height,
            Projection::Perspective { .. } => panic!("orthographic"),
        };
        view.moved(Some(tiny.clone()));
        assert!((height(&view) - 536f64.sqrt() * 400.0 / 524_288.0 * 1.01).abs() < 1e-15);
        let mut unmeasured = body(0.03125, [0.03125, 0.0625], None);
        unmeasured.mesh = Arc::new(Mesh {
            positions: vec![[20.0, 10.0, 5.0], [100.0, 10.0, 5.0]],
            edge_points: vec![[20.0, 10.0, 5.0], [200.0, 10.0, 5.0]],
            ..(*unmeasured.mesh).clone()
        });
        let mut view = view_with(&[(1, unmeasured)], 3);
        view.moved(Some(tiny));
        // Its edge's point at (200, 10, 5) is the farthest: 180 mm.
        assert!((height(&view) - 180.0 * 400.0 / 524_288.0 * 1.01).abs() < 1e-15);
    }

    fn hidden_far() -> (ViewBody, FeatureId) {
        let far = FeatureId::new("a91cw0de").unwrap();
        (
            ViewBody {
                feature: far.clone(),
                ..body(0.03125, [0.03125, 0.0625], Some([[1e4; 3], [1.1e4; 3]]))
            },
            far,
        )
    }

    #[test]
    fn a_new_tables_clamp_and_a_zooms_stop_are_by_the_bodies_drawn() {
        // The cut, and a body 10 m off that the browser hides: the least from (20, 10, 5) is
        // the cut's alone, √536 · 1e-5 · 1.01, both as a table comes and as a zoom stops.
        let least = 536f64.sqrt() * 400.0 / 524_288.0 * 1.01;
        let height = |view: &View| match view.camera.projection {
            Projection::Orthographic { view_height } => view_height,
            Projection::Perspective { .. } => panic!("orthographic"),
        };
        let (far, id) = hidden_far();
        let mut view = View {
            hidden: [id.clone()].into_iter().collect(),
            rows: 400,
            ..View::default()
        };
        view.set_camera(front(1e-6));
        let mesh = Arc::clone(&body(0.03125, [0.03125, 0.0625], CUT).mesh);
        view.take_bodies(vec![
            meshed(1, 1, 0.03125, Some(Arc::clone(&mesh))),
            MeshedBody {
                feature: id,
                bounds: far.bounds,
                ..meshed(2, 2, 0.03125, Some(mesh))
            },
        ]);
        assert!((height(&view) - least).abs() < 1e-15, "{}", height(&view));
        // From four times the least, two octaves in reach it, the far body hidden; with it
        // counted, the zoom would stop far above.
        view.camera = front(4.0 * least);
        view.zoom(400.0, [0.0, 0.0], 400.0);
        assert!((height(&view) - least).abs() < 1e-15, "{}", height(&view));
    }

    #[test]
    fn a_retry_due_with_no_read_wanted_ends_its_run() {
        // Three refusals (the next in 1 s); due with the view back where the last answer
        // was, no read is wanted and the run ends: the next refusal waits 250 ms again.
        let at = now();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        view.answered = Some(asked(&view, 3));
        view.camera = front(40.0);
        for _ in 0..3 {
            let request = asked(&view, 3);
            view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), at);
        }
        assert_eq!(view.wake_in(at), Some(Duration::from_secs(1)));
        view.camera = front(80.0);
        let due = at + Duration::from_secs(1);
        assert_eq!(view.wanted(revision(3), [1000, 1000], true, due), None);
        view.ended_if_due(true, due);
        assert!(view.retry.is_none());
        view.camera = front(40.0);
        let request = asked(&view, 3);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), due);
        assert_eq!(view.wake_in(due), Some(Duration::from_millis(250)));
    }

    #[test]
    fn a_retry_falling_due_while_the_view_moves_doesnt_end_its_run() {
        // Refused at revision 3, the bodies of 3 (front(40)'s 0.02 is out of their serves).
        // At +250 ms the view is moving (front(30), not settled): no read, and the run goes
        // on; settled there, it's read and refused, and the next waits 500 ms, not 250.
        let at = now();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        view.camera = front(40.0);
        let request = asked(&view, 3);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), at);
        let due = at + Duration::from_millis(250);
        view.camera = front(30.0);
        assert_eq!(view.wanted(revision(3), [1000, 1000], false, due), None);
        view.ended_if_due(false, due);
        assert!(view.retry.is_some());
        let settled = view.wanted(revision(3), [1000, 1000], true, due).unwrap();
        view.took(settled, false, refusal(ErrorCode::WorkerCrashed, None), due);
        assert_eq!(view.wake_in(due), Some(Duration::from_millis(500)));
    }

    /// A left click at `at` (points), pressed and let go at `time`.
    fn left_click(clicks: &mut LeftClicks, at: Pos2, time: f64) -> bool {
        let options = egui::InputOptions::default();
        clicks.pressed = Some(Pressed::new(at, time));
        clicks.released(at, time, &options)
    }

    #[test]
    fn a_press_is_a_click_within_six_points_and_eight_tenths_of_a_second() {
        // egui's defaults: 6 points, 0.8 s.
        let options = egui::InputOptions::default();
        assert_eq!(options.max_click_dist, 6.0);
        assert_eq!(options.max_click_duration, 0.8);
        let at = egui::pos2(100.0, 100.0);
        let pressed = Pressed::new(at, 0.0);
        assert!(pressed.clicked(egui::pos2(106.0, 100.0), 0.0, &options));
        assert!(!pressed.clicked(egui::pos2(106.5, 100.0), 0.0, &options));
        assert!(pressed.clicked(at, 0.8, &options));
        assert!(!pressed.clicked(at, 0.85, &options));
        // Gone 7 points away and come back: no click.
        let mut back = pressed;
        back.moved(egui::pos2(100.0, 107.0), &options);
        back.moved(egui::pos2(100.0, 101.0), &options);
        assert!(!back.clicked(at, 0.0, &options));
        // Moves within the distance keep it one.
        let mut near = pressed;
        near.moved(egui::pos2(104.0, 104.0), &options);
        assert!(near.clicked(at, 0.0, &options));
    }

    #[test]
    fn left_clicks_are_counted_as_egui_counts_them_a_second_within_its_delay_a_double() {
        let at = egui::pos2(100.0, 100.0);
        let mut clicks = LeftClicks::default();
        // A release with no press is no click.
        assert!(!clicks.released(at, 0.0, &egui::InputOptions::default()));
        // Quick clicks 0.1 s apart (egui's delay is 0.3 s): single, double, then triples
        // (each within 0.6 s of the one two before), never doubles.
        let counted: Vec<bool> = (0..4)
            .map(|n| left_click(&mut clicks, at, 10.0 + 0.1 * f64::from(n)))
            .collect();
        assert_eq!(counted, [false, true, false, false]);
        // Well after, two clicks 0.29 s apart are a double; 0.31 s, not.
        let mut clicks = LeftClicks::default();
        assert!(!left_click(&mut clicks, at, 20.0));
        assert!(left_click(&mut clicks, at, 20.29));
        let mut clicks = LeftClicks::default();
        assert!(!left_click(&mut clicks, at, 30.0));
        assert!(!left_click(&mut clicks, at, 30.31));
        // Within the delay but 6 points from the last: no double; 5.9, a double.
        let mut clicks = LeftClicks::default();
        assert!(!left_click(&mut clicks, at, 40.0));
        assert!(!left_click(&mut clicks, egui::pos2(106.0, 100.0), 40.1));
        let mut clicks = LeftClicks::default();
        assert!(!left_click(&mut clicks, at, 50.0));
        assert!(left_click(&mut clicks, egui::pos2(105.9, 100.0), 50.1));
        // The delays' ends, at times exactly representable: clicks at 0.0 and 0.3 (egui's
        // delay) aren't a double; at 0.0, 0.35, and 0.6, the third is a double, not a
        // triple (0.3 · 2.0 is exactly 0.6).
        let mut clicks = LeftClicks::default();
        assert!(!left_click(&mut clicks, at, 0.0));
        assert!(!left_click(&mut clicks, at, 0.3));
        let mut clicks = LeftClicks::default();
        assert!(!left_click(&mut clicks, at, 0.0));
        assert!(!left_click(&mut clicks, at, 0.35));
        assert!(left_click(&mut clicks, at, 0.6));
        // A drag between two quick clicks is no click, and leaves the count as it was:
        // the second is a double, as egui has it.
        let mut clicks = LeftClicks::default();
        assert!(!left_click(&mut clicks, at, 60.0));
        clicks.pressed = Some(Pressed::new(at, 60.05));
        let options = egui::InputOptions::default();
        assert!(!clicks.released(egui::pos2(120.0, 100.0), 60.1, &options));
        assert!(left_click(&mut clicks, at, 60.15));
    }
}
