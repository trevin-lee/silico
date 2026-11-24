//! The viewport (docs/design/m5.0-app.md, sections 2, 3, 8, and 10; M5.6): the canvas that
//! draws the tab's published model through a renderer ([`crate::renderer`]), under the tab's
//! own camera (decision 29), with Fusion's navigation (the wheel, the middle button, F6),
//! its ViewCube at the canvas's top right, its navigation bar at its bottom centre, and the
//! triad and the view label at its bottom left ([`crate::triad`]).
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
    self, Align, Layout, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetType,
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
use silico_core::views::{NAMED_VIEWS, NamedView, half_pixel_at, meshing_depth};
use silico_protocol::{Camera, MAX_RENDER_SIDE, Mesh, Projection};

use crate::bar;
use crate::icons::Icon;
use crate::menu::{self, Checked};
use crate::navigate::{self, HOME, NO_BOX};
use crate::renderer::{Change, Element, FrameData, Picking, PixelRect, Present, Tint};
use crate::shortcuts::Command;
use crate::tab::Tab;
use crate::theme;
use crate::transition::{Motion, Step, Transition};

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
    /// Its faces, edges, and vertices, in the mesh's order, each named (`Face1` is the
    /// first).
    pub faces: Vec<MeshElement>,
    pub edges: Vec<MeshElement>,
    pub vertices: Vec<MeshElement>,
}

impl ViewBody {
    /// Whether `element` is one of its mesh's faces, edges, or vertices.
    fn has(&self, element: Element) -> bool {
        let (number, count) = match element {
            Element::Face(n) => (n, self.mesh.face_triangles.len()),
            Element::Edge(n) => (n, self.mesh.edge_point_counts.len()),
            Element::Vertex(n) => (n, self.mesh.vertex_points.len()),
        };
        usize::try_from(number).is_ok_and(|n| (1..=count).contains(&n))
    }

    /// `element`'s name and stability, as the element map gives them (M5.7): its face
    /// `Face(n)` is `faces[n - 1]`, its edge `Edge(n)` `edges[n - 1]`, its vertex
    /// `Vertex(n)` `vertices[n - 1]`, the map numbering them from 1 in the mesh's order;
    /// none for a number it hasn't.
    pub fn name_of(&self, element: Element) -> Option<&MeshElement> {
        let (number, named) = match element {
            Element::Face(n) => (n, &self.faces),
            Element::Edge(n) => (n, &self.edges),
            Element::Vertex(n) => (n, &self.vertices),
        };
        let at = usize::try_from(number).ok()?.checked_sub(1)?;
        named.get(at)
    }

    /// Vertex `n`'s point (from 1), as its mesh holds it; none for a number it hasn't.
    fn vertex_point(&self, n: u32) -> Option<[f64; 3]> {
        let at = usize::try_from(n).ok()?.checked_sub(1)?;
        self.mesh.vertex_points.get(at).copied()
    }
}

/// A vertex drawn over the bodies: its point, and whether it's selected (else hovered).
type Dot = ([f64; 3], bool);

/// The vertices of `highlights` (selected) and `hovered` among `bodies`, each with its
/// point: the hovered first, so a selected one is drawn over it, as the prototype draws
/// them (interact.js, `overlay`'s `dot`).
fn vertex_dots(
    bodies: &BTreeMap<u32, ViewBody>,
    highlights: &[(u32, Element)],
    hovered: &[(u32, Element)],
) -> Vec<Dot> {
    let point = |&(body, element): &(u32, Element)| match element {
        Element::Vertex(n) => bodies.get(&body)?.vertex_point(n),
        Element::Face(_) | Element::Edge(_) => None,
    };
    let hovered = hovered.iter().filter_map(point).map(|p| (p, false));
    let selected = highlights.iter().filter_map(point).map(|p| (p, true));
    hovered.chain(selected).collect()
}

/// Each of `dots` painted where the renderer draws its point ([`dots_at`]).
fn paint_vertex_dots(
    painter: &egui::Painter,
    rect: Rect,
    camera: &Camera,
    size_px: [u32; 2],
    ppp: f32,
    dots: &[Dot],
) {
    for (at, selected) in dots_at(rect, camera, size_px, ppp, dots) {
        paint_vertex(painter, at, selected);
    }
}

/// Where each of `dots` is drawn, in points, in the viewport `rect`, `size_px` physical
/// pixels at `ppp` a point, and whether it's selected: where the renderer draws its point
/// ([`silico_render::render::project`], the renderer's own projection), so the dot sits on
/// the disc that picks it. None for a point nearer a perspective eye than the renderer
/// draws, or one whose place in points isn't finite. One far off the viewport is kept, for
/// the painter to clip.
fn dots_at(
    rect: Rect,
    camera: &Camera,
    size_px: [u32; 2],
    ppp: f32,
    dots: &[Dot],
) -> Vec<(Pos2, bool)> {
    dots.iter()
        .filter_map(|&(point, selected)| {
            let [x, y] = silico_render::render::project(camera, size_px, point)?;
            // Physical pixels from the viewport's corner, to points.
            let at = rect.min + Vec2::new(x as f32, y as f32) / ppp;
            (at.x.is_finite() && at.y.is_finite()).then_some((at, selected))
        })
        .collect()
}

/// A vertex's dot at `at`, in points: the prototype's (interact.js, `overlay`'s `dot`), a
/// disc of [`theme::VIEWPORT_VERTEX_RADIUS`] in the selection's or the hover's colour,
/// ringed in [`theme::VIEWPORT_VERTEX_RING_COLOR`]. The visuals the UI port replaces.
fn paint_vertex(painter: &egui::Painter, at: Pos2, selected: bool) {
    let fill = if selected {
        theme::VIEWPORT_SELECTION_VERTEX
    } else {
        theme::VIEWPORT_HOVER_VERTEX
    };
    painter.circle(
        at,
        theme::VIEWPORT_VERTEX_RADIUS,
        theme::color32(fill),
        Stroke::new(
            theme::VIEWPORT_VERTEX_RING,
            theme::color32(theme::VIEWPORT_VERTEX_RING_COLOR),
        ),
    );
}

/// What the viewport is this frame, for what draws over it and picks in it (sketch mode,
/// M5.7): the tab's session; where it is in the window, in points; the camera; the size in
/// physical pixels; the display's scale; the pointer, in physical pixels from the
/// viewport's top left, if it's over the viewport; the left button's events and the
/// right button's clicks the pane didn't take itself; and the time, as the drawing's clock
/// says it.
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
    /// The bodies the frame drew, by number, ascending: those of the view's table the
    /// browser doesn't hide and the renderer holds (none it refused), as the renderer is
    /// given them, each its mesh's (`View::bodies`). What Select Other casts on.
    pub drawn: Vec<u32>,
    /// When the frame was drawn, by the clock the viewport's reads wait on (a test's, moved
    /// by hand, through [`crate::App::presenting_clocked`]).
    pub now: Instant,
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
    /// the drag, a modal came over the window, another press came with no release before it
    /// (the window lost one), or the tab stopped being shown. What the stream did is undone,
    /// not done; it's at where the stream was last (its press, or its last move). The pointer
    /// leaving the window cancels nothing: the drag goes on past its edge, and its release,
    /// wherever it comes, ends it.
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

/// The canvas's own left clicks, counted as egui counts the pointer's (`crate::clicks`, #452):
/// its press under way, and its clicks' count. A click off the canvas isn't counted, so a
/// double-click on it just after one isn't taken as a triple.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct LeftClicks {
    pressed: Option<Pressed>,
    count: crate::clicks::Count,
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
        self.count.click(at, time, options)
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

/// The boxes of what a frame draws besides the bodies, each `[low, high]` in the document's
/// coordinates.
#[derive(Debug, Clone, Default, PartialEq)]
struct DrawnBoxes {
    /// Each shown sketch's lines', as drawn: in the reach a view is clamped by, as the
    /// renderer takes them into its own.
    sketch_lines: Vec<[[f64; 3]; 2]>,
    /// Each shown sketch's whole, as sketch mode fits its own (`sketch::bounds::model_box`),
    /// the same at every scale: in what a fit takes ([`View::fit_bounds`]).
    sketch_fits: Vec<[[f64; 3]; 2]>,
    /// The shown origin planes': in the reach a view is clamped by, never fitted.
    planes: Vec<[[f64; 3]; 2]>,
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
    /// The table the frame before this one drew: [`table`](Self::table) as this frame
    /// began, before its poll.
    table_before: u64,
    reading: Option<Reading>,
    /// The read out's answer, waited for between frames by a test
    /// ([`read_answered_within`](Self::read_answered_within)): taken by the next poll.
    came: Option<(Asked, bool, Response<MeshesRead>)>,
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
    /// The boxes of what the last frame drew besides the bodies: the shown sketches' and the
    /// origin planes' ([`DrawnBoxes`]).
    drawn: DrawnBoxes,
    /// The camera the tab opened at, Home's (the first read's, or, with no body to fit, the
    /// cube's), while the view is still there: a shown sketch's box coming or changing then
    /// fits Home again with it, so the view a part opens at is the one Home gives.
    opening: Option<Camera>,
    notice: Option<Notice>,
    /// A held mesh an answer gave none of wasn't found: the next read holds none.
    hold_none: bool,
    reads: u64,
    /// The camera and size of the last frame, to tell when the view has settled.
    last: Option<(Camera, [u32; 2])>,
    drag: Option<Drag>,
    /// The navigation bar's tool, if one is chosen.
    pub tool: Option<Navigation>,
    /// Whether the ground grid is shown ([`crate::grid`]): the navigation bar's Grid, on at
    /// first, as the prototype's is. A sketch open hides it whatever this says.
    pub grid: bool,
    /// The elements drawn highlighted, set from outside (M5.7's selection): passed through,
    /// but for one of a body the frame doesn't draw, or that its mesh hasn't.
    pub highlights: BTreeSet<(u32, Element)>,
    /// The elements drawn hovered (M5.7's pre-selection), set from outside, passed through
    /// as `highlights` are.
    pub hovered: BTreeSet<(u32, Element)>,
    /// The faces of the features selected (a timeline item: the faces it made; a body by
    /// its browser row: its every face), set from outside (M5.7's selection): tinted as a
    /// feature hovered in the timeline is, while none is ([`maker_faces`](Self::maker_faces)).
    pub selected_feature_faces: BTreeSet<(u32, Element)>,
    /// The radius, in points, within which a pick takes a vertex, if it takes vertices,
    /// set from outside (the selection's filter, M5.7): the renderer's
    /// [`crate::renderer::Frame::vertices`]. None picks none.
    pub vertices: Option<f32>,
    /// Whether a pick takes edges, set from outside (the selection's filter, M5.7): the
    /// renderer's [`crate::renderer::Frame::edges`], which draws each edge near a pick's
    /// square again toward the eye only then, so with edges off an edge is picked where it's
    /// drawn alone, a concave edge's stroke as a convex one's (and the selection takes the
    /// face cast under it). On, as the selection's filter starts.
    pub edges: bool,
    /// The broken reference's candidate whose card row the pointer is over (flow 6b), set
    /// from outside: drawn with the hover, in [`theme::VIEWPORT_CANDIDATE`] for that frame.
    ///
    /// SEAM: the prototype lights every candidate while its card shows, in
    /// [`theme::VIEWPORT_CANDIDATE`] [`theme::VIEWPORT_CANDIDATE_WIDTH`] wide, and the hovered
    /// one [`theme::VIEWPORT_CANDIDATE_HOT_WIDTH`] wide; the renderer has two outline layers
    /// (the selection's and the hover's) of one width, so the one hovered is lit alone, on
    /// the hover's layer, until it has a third.
    pub candidate: Option<(u32, Element)>,
    /// The feature hovered in the timeline (DESIGN.md, *Hover a feature, see its geometry*;
    /// the prototype's `hoverFeature`): the faces it made, as each face's name says
    /// ([`Name::feature`]), are tinted while it's hovered, in place of the features
    /// selected's, in [`theme::VIEWPORT_FEATURE_HOVER_TINT`] at
    /// [`theme::VIEWPORT_FEATURE_HOVER_TINT_AMOUNT`] ([`feature_tint`]), but any selected
    /// or hovered ([`tinted_faces`]).
    ///
    /// [`Name::feature`]: silico_core::naming::Name::feature
    pub linked: Option<FeatureId>,
    /// The feature that made what the canvas's hover is over, as the timeline read it when
    /// it last drew (it rings that feature's item, `.hovered-from-canvas`): set by the
    /// timeline, drawn before the canvas, each frame. Once the canvas's hover names another
    /// feature, the viewport asks for the next frame at once, so the timeline catches up.
    pub ringed_feature: Option<FeatureId>,
    /// The origin plane whose browser row the pointer is on, set by the browser each frame
    /// it's drawn: drawn hovered, its eye open or not (`crate::planes`).
    pub plane_hovered: Option<silico_core::document::OriginPlane>,
    /// The origin planes the last frame drew, in `crate::planes::ORIGIN_PLANES`' order: what
    /// Create Sketch's pick casts on (`crate::plane_pick`), as the prototype's `VP.pickPlane`
    /// casts on the planes it shows.
    planes_drawn: Vec<silico_core::document::OriginPlane>,
    picking: Option<PixelRect>,
    picked: Option<Picking>,
    /// The serial of the newest frame that carried a pick of this view's: an answer to a
    /// frame after it is to a pick asked since.
    last_asked: Option<u64>,
    frame: Option<ViewFrame>,
    /// The pane's height in pixels, as its last frame had it: an orthographic view's least
    /// is the renderer's at that height (`views::least_view_height`). 1 before the first
    /// frame, so a camera clamped then takes the 1-row floor until that frame re-clamps it.
    rows: u32,
    /// A view a shortcut asked for (F6, Home, Ctrl+1 to Ctrl+7: `crate::shortcuts`), or a
    /// camera to go back to, given at the pane's next navigation, which knows the canvas's
    /// size.
    asked: Option<Wanted>,
    /// The tab's count of published models, as the tab last told it, and as it was when the
    /// bodies held were asked for: a model published again at the same revision (a fresh
    /// check's, Ctrl+Shift+B) makes them stale, as a new revision does.
    published: u64,
    read_published: u64,
    /// Whether a menu over the canvas holds the wheel (Select Other's, which steps through
    /// its rows with it, `crate::select_other`): the wheel doesn't zoom while it does. Held
    /// from the frame after the menu opens (the tab's overlay comes after the pane's
    /// navigation), and let go as it closes.
    pub(crate) wheel_held: bool,
    /// What sketch mode adds to a fit, as the tab last set it.
    framing: navigate::Framing,
    /// A view transition under way (`crate::transition`): Home, Fit, a named view, a
    /// ViewCube click, or Look At easing to its view, `camera` the camera drawn this frame.
    transition: Option<Transition>,
}

/// The views the registry's view commands give, by their keys (F6, Home, Ctrl+1 to Ctrl+7)
/// and by the ViewCube's menu (Home view and the named views): one table, so neither can
/// give another view than the other.
pub const VIEW_COMMANDS: [(Command, ViewAsk); 9] = [
    (Command::Fit, ViewAsk::Fit),
    (Command::Home, ViewAsk::Named(HOME)),
    (Command::Front, ViewAsk::Named(NamedView::Front)),
    (Command::Back, ViewAsk::Named(NamedView::Back)),
    (Command::Left, ViewAsk::Named(NamedView::Left)),
    (Command::Right, ViewAsk::Named(NamedView::Right)),
    (Command::Top, ViewAsk::Named(NamedView::Top)),
    (Command::Bottom, ViewAsk::Named(NamedView::Bottom)),
    (Command::Isometric, ViewAsk::Named(navigate::ISOMETRIC)),
];

/// What the view is asked to go to at its next navigation: a view a shortcut asks
/// ([`View::ask`]), or a camera it had before ([`View::return_to`]).
#[derive(Debug, Clone, PartialEq)]
enum Wanted {
    Ask(ViewAsk),
    Camera(Camera),
}

/// A view a shortcut asks of the tab's viewport. Each fits the bodies and the shown
/// sketches (`View::fit_bounds`) with what the view's [`navigate::Framing`] adds in sketch
/// mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewAsk {
    /// The bodies and the shown sketches fitted, looking as it looks (F6, the navigation
    /// bar's Fit).
    Fit,
    /// A named view of the bodies and the shown sketches, as the ViewCube gives it (Home, a
    /// face, an edge, a corner).
    Named(NamedView),
    /// Look At (Ctrl+8): the view square to a plane, looking from `toward`, its unit
    /// normal toward the eye, with `up` up the image ([`navigate::look_at`]).
    LookAt { toward: [f64; 3], up: [f64; 3] },
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
            table_before: 0,
            reading: None,
            came: None,
            refused: None,
            retry: None,
            said: None,
            said_for: None,
            hidden: BTreeSet::new(),
            drawn: DrawnBoxes::default(),
            opening: None,
            notice: None,
            hold_none: false,
            reads: 0,
            last: None,
            drag: None,
            tool: None,
            grid: true,
            highlights: BTreeSet::new(),
            hovered: BTreeSet::new(),
            selected_feature_faces: BTreeSet::new(),
            vertices: None,
            edges: true,
            candidate: None,
            linked: None,
            ringed_feature: None,
            plane_hovered: None,
            planes_drawn: Vec::new(),
            picking: None,
            picked: None,
            last_asked: None,
            frame: None,
            rows: 1,
            asked: None,
            published: 0,
            read_published: 0,
            wheel_held: false,
            framing: navigate::Framing::default(),
            transition: None,
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
        self.asked = Some(Wanted::Ask(ask));
    }

    /// Back to `camera`, a camera the view had (the one before sketch mode turned it to its
    /// sketch): given at the pane's next navigation as a view worked out whole is, eased to
    /// and raised to the least the bodies allow, in the projection the view has then, in
    /// place of any view asked before it that hasn't been given.
    pub fn return_to(&mut self, camera: Camera) {
        self.asked = Some(Wanted::Camera(camera));
    }

    /// The camera the view is heading for: the view a transition under way eases to, or the
    /// camera, with none under way. A view asked and not yet given (at the pane's next
    /// navigation) isn't in it.
    pub fn heading(&self) -> &Camera {
        self.transition
            .as_ref()
            .map_or(&self.camera, Transition::to)
    }

    /// Whether the camera has been chosen: by the person, a view asked, or the first read's
    /// Home. Until it is, the camera is a stand-in, fitted to nothing.
    pub fn placed(&self) -> bool {
        self.placed
    }

    /// What sketch mode adds to a fit from now on ([`navigate::Framing`]): the tab's, each
    /// frame.
    pub fn frame_with(&mut self, framing: navigate::Framing) {
        self.framing = framing;
    }

    /// Looks through `camera`, as a control of the person's does: at once, ending any
    /// transition under way.
    pub fn set_camera(&mut self, camera: Camera) {
        self.transition = None;
        self.camera = camera;
        self.placed = true;
    }

    /// The view transition under way, if one is: [`camera`](Self::camera) is the camera it
    /// has reached, the one drawn.
    pub fn transition(&self) -> Option<&Transition> {
        self.transition.as_ref()
    }

    /// The camera the view label names: the one drawn, but while a transition is under way
    /// the one it named before it began, as the prototype's label is written again only once
    /// its transition ends (interact.js's `onView`, called from `stepAnim` at its end).
    pub fn labelled_camera(&self) -> &Camera {
        self.transition
            .as_ref()
            .map_or(&self.camera, Transition::labelled)
    }

    /// The transition under way moved on to `now`, egui's clock: the camera where it has
    /// reached, or, once its time is up, the view it was asked for, exactly, and it ends. A
    /// press on the canvas this frame (`pressed`) ends it while it's still moving, the camera
    /// left where it was drawn last frame, what the person pressed on; once its time is up,
    /// the press is on the view asked for. Each camera raised to the least the bodies allow,
    /// as every camera a control makes is (`navigate::clamped`), which leaves the view asked
    /// for, already raised, as it was unless the bodies changed meanwhile.
    fn advance(&mut self, now: f64, pressed: bool) {
        let Some(transition) = &self.transition else {
            return;
        };
        let camera = match transition.at(now) {
            Step::Moving(_) if pressed => {
                self.transition = None;
                return;
            }
            Step::Moving(camera) => camera,
            Step::Ended(camera) => {
                self.transition = None;
                camera
            }
        };
        self.camera = navigate::clamped(camera, self.rows, self.drawn_boxes());
    }

    /// The bodies whose meshes the tab holds, by number.
    pub fn bodies(&self) -> &BTreeMap<u32, ViewBody> {
        &self.bodies
    }

    /// The faces the feature hovered in the timeline made ([`linked`](Self::linked)), in
    /// every body that holds one, as each face's name says: none with none hovered.
    pub fn linked_faces(&self) -> Vec<(u32, Element)> {
        match &self.linked {
            Some(feature) => faces_of(feature, &self.bodies, false),
            None => Vec::new(),
        }
    }

    /// The faces tinted as their maker's (the prototype's `maker`, `hoverFeature ||
    /// selFeature`): the feature hovered in the timeline's, if one is, even if it made none;
    /// else the features selected's.
    pub fn maker_faces(&self) -> Vec<(u32, Element)> {
        match self.linked {
            Some(_) => self.linked_faces(),
            None => self.selected_feature_faces.iter().copied().collect(),
        }
    }

    /// The published revision the bodies were read from, once a read has answered.
    pub fn model(&self) -> Option<Revision> {
        self.model
    }

    /// Which published model the bodies were read from, once a read has answered: the
    /// tab's count of published models when it was asked, and the revision it read. What
    /// the selection names a pick from and draws its highlights on only when it's the
    /// model the selection is read against (M5.7): a pick of these bodies named, or an
    /// element's number drawn on them, as another model has it would be another element.
    /// A read asked under one count and answered from a model published since gives that
    /// model's revision beside the older count: it matches no model the tab holds, and is
    /// read again.
    pub fn bodies_of(&self) -> Option<(u64, Revision)> {
        self.model.map(|revision| (self.read_published, revision))
    }

    /// The origin planes the last frame drew, in `crate::planes::ORIGIN_PLANES`' order.
    pub fn planes_drawn(&self) -> &[silico_core::document::OriginPlane] {
        &self.planes_drawn
    }

    /// How many `meshes` reads the tab has started.
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// Whether a `meshes` read is in flight.
    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Whether the meshes of `revision` are neither being read nor coming by themselves: no
    /// read is in flight, and the last of that revision was refused, for what it asked (not
    /// asked again until the view differs) or for the moment (asked again only after a
    /// delay, and maybe refused again). What waits for them (M5.7's click) waits in vain.
    pub fn refused_of(&self, revision: Revision) -> bool {
        self.reading.is_none()
            && (self
                .refused
                .as_ref()
                .is_some_and(|refused| refused.revision == revision)
                || self
                    .retry
                    .as_ref()
                    .is_some_and(|retry| retry.asked.revision == revision))
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

    /// The pick asked for the next frame, if one is (M5.7: the box select's press, which
    /// asks its own only in place of the hover's).
    pub fn asking(&self) -> Option<PixelRect> {
        self.picking
    }

    /// The answer to the last pick asked, once it has come, if it's of the bodies the tab
    /// holds now.
    pub fn picked(&self) -> Option<Picking> {
        self.picked
    }

    /// The serial of the newest frame that carried one of its picks ([`Picking::frame`]
    /// answers it), if one has: what asks a pick again (M5.7's click, whose request a
    /// discarded egui pass consumes without painting) tells its own answers by, those to a
    /// frame after the newest before it asked.
    pub fn last_asked(&self) -> Option<u64> {
        self.last_asked
    }

    /// Whether a drag navigates the view (the middle button's, a navigation tool's, the
    /// ViewCube's): the hover isn't asked meanwhile.
    pub fn navigating(&self) -> bool {
        self.drag.is_some()
    }

    /// Which bodies table it holds: one more each time the table is made again (a new
    /// read's bodies), so what's worked out from the bodies is worked out again.
    pub fn table(&self) -> u64 {
        self.generation
    }

    /// Whether this frame took a new table of bodies: the frame before drew another, so
    /// the clicks this frame hands on were made on that one, not on the table they'd be cast
    /// on now.
    pub fn table_taken(&self) -> bool {
        self.generation != self.table_before
    }

    /// As if bodies read from `of` had come, and the answer to the pick of frame `picking`
    /// had: a selection's unit tests' view.
    #[cfg(test)]
    pub(crate) fn answered_as(&mut self, of: (u64, Revision), picking: Picking) {
        self.read_published = of.0;
        self.model = Some(of.1);
        self.last_asked = Some(picking.frame);
        self.picked = Some(picking);
    }

    /// As if another table of bodies had been read, of the same model: a selection's unit
    /// tests' view.
    #[cfg(test)]
    pub(crate) fn tabled_again(&mut self) {
        self.generation += 1;
    }

    /// A view holding `bodies`, read from no model: a unit test's.
    #[cfg(test)]
    pub(crate) fn holding(bodies: BTreeMap<u32, ViewBody>) -> View {
        View {
            bodies,
            ..View::default()
        }
    }

    /// As if the last frame drew the origin planes `planes`: a unit test's, of what Create
    /// Sketch's pick casts on (`crate::plane_pick`).
    #[cfg(test)]
    pub(crate) fn drew_planes(&mut self, planes: &[silico_core::document::OriginPlane]) {
        self.planes_drawn = planes.to_vec();
    }

    /// The pick asked of the renderer and not yet given it, taken (as a frame takes it): a
    /// unit test's, to see whether another is asked after.
    #[cfg(test)]
    pub(crate) fn take_asked(&mut self) -> Option<PixelRect> {
        self.picking.take()
    }

    /// Waits up to `timeout` for the read out to answer, its answer kept for the next
    /// frame's poll to take: a test's, so a new table is taken in the frame it chooses, as
    /// any frame may take one. Whether a read was out and has answered. For the crate's tests
    /// only.
    #[doc(hidden)]
    pub fn read_answered_within(&mut self, timeout: Duration) -> bool {
        let Some(reading) = self.reading.take() else {
            return false;
        };
        match reading.running.wait_timeout(timeout) {
            Ok(response) => {
                self.came = Some((reading.asked, reading.named, response));
                true
            }
            Err(running) => {
                self.reading = Some(Reading { running, ..reading });
                false
            }
        }
    }

    /// The read in flight polled: its answer taken if it has come.
    fn poll(&mut self, now: Instant) {
        if let Some((asked, named, response)) = self.came.take() {
            self.took(asked, named, response, now);
            return;
        }
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
                let opening = named && !self.placed;
                if opening {
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
                let size = asked.size;
                self.answered = Some(asked);
                self.take_bodies(read.bodies);
                if opening {
                    self.opened(size);
                }
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
                let opening = !self.placed;
                self.placed = true;
                if opening {
                    self.opened(asked.size);
                }
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
                    vertices: body.vertices,
                },
            );
        }
        self.bodies = table;
        self.generation = self.generation.wrapping_add(1);
        // A view zoomed in before these bodies came (or as close as smaller ones allowed)
        // raised to the least they allow, so every control can move it on.
        self.camera = navigate::clamped(self.camera.clone(), self.rows, self.drawn_boxes());
    }

    /// The boxes of what the frame draws besides the bodies, in a pane `size_px` pixels in
    /// size: if the shown sketches' changed what a fit takes while the view is still the one
    /// the tab opened at, Home fitted again with them; else, if any changed, the view raised
    /// to the least they allow with the bodies, if it's under it, as the renderer takes
    /// their reach too (an orthographic view under it would have every frame refused).
    fn take_drawn_boxes(&mut self, drawn: DrawnBoxes, size_px: [u32; 2]) {
        if drawn == self.drawn {
            return;
        }
        let before = self.fit_bounds();
        let opening = self.at_opening();
        self.drawn = drawn;
        if opening && self.fit_bounds() != before {
            self.look_from(HOME, size_px, Motion::JUMP);
            self.opening = Some(self.camera.clone());
        } else {
            self.camera = navigate::clamped(self.camera.clone(), self.rows, self.drawn_boxes());
        }
    }

    /// Whether the view is still the one the tab opened at, unmoved and moving nowhere.
    fn at_opening(&self) -> bool {
        self.transition.is_none() && self.opening.as_ref() == Some(&self.camera)
    }

    /// The tab's opening view placed, in a pane `size_px` pixels in size: Home fitted again
    /// if the shown sketches reach past what the core's Home fitted (the bodies, and the
    /// open sketch, whose framing sketch mode gives the view), and kept as the opening view.
    fn opened(&mut self, size_px: [u32; 2]) {
        let fitted =
            silico_core::views::joined(self.bounds().into_iter().chain(self.framing.sketch_box));
        if self.fit_bounds() != fitted {
            self.look_from(HOME, size_px, Motion::JUMP);
        }
        self.opening = Some(self.camera.clone());
    }

    /// Every body's box.
    fn boxes(&self) -> impl Iterator<Item = [[f64; 3]; 2]> + '_ {
        self.bodies.values().filter_map(|body| body.bounds)
    }

    /// The boxes of the bodies drawn (those the browser hides left out), as the renderer
    /// takes their reach: a body's tight box, or, for one that couldn't be measured, its
    /// mesh's own extent; and the shown sketches' lines' boxes and the planes', which it
    /// takes too.
    fn drawn_boxes(&self) -> impl Iterator<Item = [[f64; 3]; 2]> + '_ {
        self.bodies
            .values()
            .filter(|body| !self.hidden.contains(&body.feature))
            .filter_map(|body| body.bounds.or_else(|| extent(&body.mesh)))
            .chain(self.drawn.sketch_lines.iter().copied())
            .chain(self.drawn.planes.iter().copied())
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
    /// bodies allow if it's under it (`navigate::clamped`), at once, ending any transition
    /// under way where it had reached. Whether it was taken.
    fn moved(&mut self, camera: Option<Camera>) -> bool {
        let Some(camera) = camera else {
            return false;
        };
        self.transition = None;
        self.camera = navigate::clamped(camera, self.rows, self.drawn_boxes());
        self.placed = true;
        true
    }

    /// The view worked out whole as `camera` (Home's, a fit's, a named view's, Look At's), if
    /// one was, raised to the least the bodies allow as [`moved`](Self::moved) raises it, and
    /// moved to as `motion` says: eased to from the camera as it is, from where any transition
    /// under way has reached (its label kept), or, for a jump or a camera already there,
    /// taken at once.
    fn go_to(&mut self, camera: Option<Camera>, motion: Motion) {
        let Some(camera) = camera else {
            return;
        };
        let to = navigate::clamped(camera, self.rows, self.drawn_boxes());
        self.placed = true;
        let labelled = self.transition.take().map_or_else(
            || self.camera.clone(),
            |under_way| under_way.labelled().clone(),
        );
        if motion.seconds > 0.0 && to != self.camera {
            self.transition = Some(Transition::new(self.camera.clone(), to, labelled, motion));
        } else {
            self.camera = to;
        }
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

    /// The bodies' boxes, the shown sketches' (each its read's whole, the same at every
    /// scale), and the open sketch's (its framing's), joined: what a fit, Home, a named view,
    /// and Look At take, as Fusion's Fit takes every object shown. The origin planes aren't
    /// in it: they're the prototype's fixed 50 mm whatever the part's size (DESIGN.md), so a
    /// part smaller than them would be fitted to them.
    fn fit_bounds(&self) -> Option<[[f64; 3]; 2]> {
        silico_core::views::joined(
            self.bounds()
                .into_iter()
                .chain(self.drawn.sketch_fits.iter().copied())
                .chain(self.framing.sketch_box),
        )
    }

    /// The named view `view` of [`fit_bounds`](Self::fit_bounds) (a cube's, with none, or
    /// with a box that has no extent to fit), clear of sketch mode's bands, in the view's
    /// projection.
    fn look_from(&mut self, view: NamedView, size_px: [u32; 2], motion: Motion) {
        let named =
            |bounds| navigate::named_in(view, bounds, size_px[0], size_px[1], self.framing.inset);
        let camera = named(self.fit_bounds().unwrap_or(NO_BOX))
            .or_else(|| named(NO_BOX))
            .and_then(|camera| navigate::projected(&camera, self.perspective));
        self.go_to(camera, motion);
    }

    /// Look At: the view square to a plane from `toward`, `up` up the image, fitted to the
    /// boxes a named view fits (a cube's, with none, or with a box that has no extent to
    /// fit), in the view's projection.
    fn look_at(&mut self, toward: [f64; 3], up: [f64; 3], size_px: [u32; 2], motion: Motion) {
        let square = |bounds| {
            navigate::look_at(
                toward,
                up,
                bounds,
                size_px[0],
                size_px[1],
                self.framing.inset,
            )
        };
        let camera = square(self.fit_bounds().unwrap_or(NO_BOX))
            .or_else(|| square(NO_BOX))
            .and_then(|camera| navigate::projected(&camera, self.perspective));
        self.go_to(camera, motion);
    }

    /// Fits the view to [`fit_bounds`](Self::fit_bounds) (F6), looking as it looks, clear of
    /// sketch mode's bands; with nothing to fit, or a box that has no extent, it stays.
    fn fit(&mut self, size_px: [u32; 2], motion: Motion) {
        if let Some(bounds) = self.fit_bounds() {
            let camera = navigate::fit_in(
                &self.camera,
                bounds,
                size_px[0],
                size_px[1],
                self.framing.inset,
            );
            self.go_to(camera, motion);
        }
    }

    /// The view `ask` asks for, given now, in a pane `size_px` pixels in size, moved to as
    /// `motion` says.
    fn give(&mut self, ask: ViewAsk, size_px: [u32; 2], motion: Motion) {
        match ask {
            ViewAsk::Named(named) => self.look_from(named, size_px, motion),
            ViewAsk::Fit => self.fit(size_px, motion),
            ViewAsk::LookAt { toward, up } => self.look_at(toward, up, size_px, motion),
        }
    }

    /// The view made perspective, or orthographic: at once, and if a transition is under way,
    /// its view, which it ends, so the view is the one asked for, in the projection asked.
    fn project(&mut self, perspective: bool) {
        let camera = navigate::projected(self.heading(), perspective);
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
    /// made it; none while a navigation tool makes a left drag navigate, and no press while
    /// a modal is over the window, which cancels what's still held after the frame's events.
    /// `held` is the pane's, which [`end_unshown`] has already ended if another tab's. Each
    /// is placed by the egui event that made it ([`Placed`]), its `seq` given by
    /// [`sequenced`].
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
        // A modal over the window takes no press (#487: nothing behind a modal takes input),
        // and cancels what's still held after the frame's events, as the focus going does.
        // In the frame a key opens it, the press already held takes that frame's moves and
        // its release, before the key or after it (the keys are read first): a release among
        // them ends its gesture as ever.
        let behind = crate::modal::behind(ui);
        let pressable = response.hovered() && self.drag.is_none() && !taken && !behind;
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
            // The canvas's own left clicks are counted, those whose press it takes, as egui
            // counts clicks but for none off it (`crate::clicks`); a stream's are handed on.
            for (place, each) in (1..).zip(&input.events) {
                match *each {
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    } => {
                        if pressable && rect.contains(pos) {
                            clicks.pressed = Some(Pressed::new(pos, time));
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
            if behind && let Some(was) = held.take() {
                let place = input.events.len() + 1;
                left.push(event(place, LeftKind::Cancel, was.at_px, input.modifiers));
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
        // A right press held as a modal comes over the window is no click, though let go
        // among the frame's events before the key that opened it (unlike the left button's
        // release): what a right click opens, a menu, closes under a modal, and one first
        // shown then would be raised over it.
        if crate::modal::behind(ui) {
            *held = None;
        }
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
        // A modal over the window ends a pan or an orbit held (the canvas inert under it
        // takes no new one), as SolidWorks ends a drag when a dialog takes the focus.
        if crate::modal::behind(ui) {
            self.drag = None;
        }
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
        if response.hovered() && !self.wheel_held {
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
        // The canvas's own middle double-click (`crate::clicks`), whatever was clicked just
        // before; its clicks counted every frame, a view asked for or not.
        let middle_double = crate::clicks::double_clicked_by(response, PointerButton::Middle);
        let motion = Motion::of(ui.ctx());
        match self.asked.take() {
            Some(Wanted::Ask(ask)) => self.give(ask, size_px, motion),
            Some(Wanted::Camera(camera)) => {
                self.go_to(navigate::projected(&camera, self.perspective), motion);
            }
            None if middle_double => self.fit(size_px, motion),
            None => {}
        }
    }
}

/// A frame's button events, each with its place: 0 for a Cancel a navigation tool forces
/// before the frame's events, `i + 1` for one the frame's `i`th egui event made, and `n + 1`
/// for a Cancel a modal forces after the frame's `n` events.
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

/// The box of `plane`'s corners.
fn plane_box(plane: &silico_render::render::Plane) -> Option<[[f64; 3]; 2]> {
    silico_protocol::box_of(plane.corners().iter()).map(|(low, high)| [low, high])
}

/// The canvas behind the bodies over `rect`: a vertical gradient from
/// [`theme::VIEWPORT_BG_TOP`] along its top to [`theme::VIEWPORT_BG_BOTTOM`] along its
/// bottom, its colours blended as egui blends a mesh's (in sRGB, as CSS's `linear-gradient`
/// does). The renderer's frames are transparent where no body is
/// ([`theme::VIEWPORT_FRAME_BG`]), so it shows there.
fn canvas_background(rect: Rect) -> egui::Shape {
    let top = theme::color32(theme::VIEWPORT_BG_TOP);
    let bottom = theme::color32(theme::VIEWPORT_BG_BOTTOM);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    egui::Shape::mesh(mesh)
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
    ui.painter().add(canvas_background(rect));
    // The ground grid's place, over the canvas and under everything else the pane paints,
    // the renderer's frame among it: filled once this frame's camera is known.
    let grid = ui.painter().add(egui::Shape::Noop);
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
    // A transition under way moved on before anything of this frame reads the camera; but a
    // press on the canvas (to select, to drag, for a menu) ends one still moving where it was
    // drawn last frame, what the person pressed on, so the press and what it's of are of one
    // view.
    let pressed = response.hovered() && ui.input(|input| input.pointer.any_pressed());
    tab.view.advance(ui.input(|input| input.time), pressed);
    let now_hidden: BTreeSet<FeatureId> = tab
        .browser
        .bodies()
        .iter()
        .filter(|row| !tab.shown(&crate::tab::Hideable::Feature(row.id.clone())))
        .map(|row| row.id.clone())
        .collect();
    let sketching = tab.in_sketch();
    // The shown sketches' lines, each from its read as the tab holds it, and none while a
    // sketch is open: sketch mode draws that one, and no other is drawn (the prototype's
    // `if (S.sketch) setSketchLines([])`, app.js:979), the lines made kept for after it. A
    // suppressed sketch has no read. Lines left to make for a frame's build budget are made
    // in the frames after, each asked for.
    let lines = if tab.sketch.is_some() {
        Vec::new()
    } else {
        let shown_sketches: Vec<(FeatureId, Arc<silico_core::command::sketch::SketchRead>)> = tab
            .browser
            .sketches()
            .iter()
            .filter(|row| tab.shown(&crate::tab::Hideable::Feature(row.id.clone())))
            .filter_map(|row| {
                let held = tab.sketch_reads.held(&row.id)?;
                Some((row.id.clone(), Arc::clone(&held.read)))
            })
            .collect();
        let lines = tab.sketch_lines.frame(
            shown_sketches.iter().map(|(id, read)| (id, read)),
            &tab.view.camera,
            rect.height(),
        );
        if tab.sketch_lines.pending() {
            ui.ctx().request_repaint();
        }
        lines
    };
    // The origin planes, by their eyes, the browser row hovered, and Create Sketch's pick's
    // if it's on (`crate::planes`), whatever mode the tab is in.
    let planes = crate::planes::drawn(
        |plane| tab.shown(&crate::tab::Hideable::Origin(crate::planes::row_of(plane))),
        tab.view.plane_hovered,
        tab.plane_pick
            .as_ref()
            .map(crate::plane_pick::PlanePick::lit_plane),
    );
    let drawn = DrawnBoxes {
        sketch_lines: lines
            .iter()
            .filter_map(|line| line.strips.bounds())
            .map(|(low, high)| [low, high])
            .collect(),
        sketch_fits: lines.iter().filter_map(|line| line.fit).collect(),
        planes: planes
            .iter()
            .filter(|drawn| drawn.shown)
            .filter_map(|drawn| plane_box(&drawn.plane))
            .collect(),
    };
    let view = &mut tab.view;
    view.take_drawn_boxes(drawn, size_px);
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
    view.table_before = view.generation;
    view.poll(now);
    match view.notice.take() {
        Some(Notice::Refused(said)) => tab.message = Some(said),
        Some(Notice::Answered(said)) if tab.message.as_deref() == Some(said.as_str()) => {
            tab.message = None;
        }
        _ => {}
    }
    // A table taken just now numbers the bodies afresh: the selection's highlights and
    // tints worked out for it before this frame is handed on, never drawn by the numbers of
    // the table before it.
    tab.light_selection();
    let view = &mut tab.view;
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
    // Ascending, each once, as the renderer takes them: the table's order.
    let bodies: Vec<u32> = view
        .bodies
        .iter()
        .filter(|(number, body)| !view.hidden.contains(&body.feature) && drawing.holds(**number))
        .map(|(number, _)| *number)
        .collect();
    view.frame = Some(ViewFrame {
        session: tab.id,
        rect,
        camera: view.camera.clone(),
        size_px,
        pixels_per_point: ppp,
        pointer_px,
        left,
        secondary,
        drawn: bodies.clone(),
        now,
    });
    // A highlight of a body not drawn, or of an element its mesh hasn't, would have the
    // renderer refuse the whole frame.
    let makers = view.maker_faces();
    let drawn = |(number, element): &(u32, Element)| {
        bodies.binary_search(number).is_ok()
            && view
                .bodies
                .get(number)
                .is_some_and(|body| body.has(*element))
    };
    let highlights: Vec<(u32, Element)> = view.highlights.iter().copied().filter(drawn).collect();
    // A candidate lit takes the hover's layer, in its own colour: the pointer is over its
    // card's row, not the canvas.
    let candidate = view.candidate.filter(drawn);
    let hovered: Vec<(u32, Element)> = view
        .hovered
        .iter()
        .copied()
        .chain(candidate)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(drawn)
        .collect();
    let tinted = tinted_faces(makers, &highlights, &hovered, drawn);
    // The vertices among them, drawn over the bodies by the pane (the renderer draws none).
    let dots = vertex_dots(&view.bodies, &highlights, &hovered);
    let mut outlines = crate::selection::outlines();
    if candidate.is_some() {
        let [r, g, b, _] = theme::VIEWPORT_CANDIDATE;
        outlines.hovered = [r, g, b];
    }
    let picking = view.picking.take();
    let serial = match picking {
        Some(_) => {
            let serial = drawing.asked(tab.id, view.generation);
            view.last_asked = Some(serial);
            serial
        }
        None => drawing.frames,
    };
    // A plane its eye shows is in the view's reach; one shown for a moment (its row hovered,
    // Create Sketch's pick on) is drawn only if the view as it is holds it, never zooming it
    // out.
    let planes: Vec<(
        silico_core::document::OriginPlane,
        silico_render::render::Plane,
    )> = planes
        .into_iter()
        .filter(|drawn| {
            drawn.shown
                || plane_box(&drawn.plane).is_some_and(|its| {
                    navigate::drawable(
                        &view.camera,
                        view.rows,
                        view.drawn_boxes().chain(std::iter::once(its)),
                    )
                })
        })
        .map(|drawn| (drawn.of, drawn.plane))
        .collect();
    view.planes_drawn = planes.iter().map(|(of, _)| *of).collect();
    let planes: Vec<silico_render::render::Plane> =
        planes.into_iter().map(|(_, plane)| plane).collect();
    let frame = FrameData {
        serial,
        camera: view.camera.clone(),
        size_px,
        pixels_per_point: ppp,
        bodies,
        highlights,
        hovered,
        outlines,
        tinted,
        tint: feature_tint(),
        picking,
        edges: view.edges,
        vertices: view.vertices,
        lines,
        planes,
    };
    if view.grid && !sketching {
        let shapes = crate::grid::shapes(&view.camera, size_px, ppp, rect);
        ui.painter().set(grid, egui::Shape::Vec(shapes));
    }
    drawing.present(ui.painter(), rect, changes, Some(frame));
    paint_vertex_dots(
        &ui.painter_at(rect),
        rect,
        &view.camera,
        size_px,
        ppp,
        &dots,
    );
    if drawing.picking() {
        // An idle egui draws no frame, and its answer is taken only in one.
        ui.ctx().request_repaint();
    }
    // The triad at the canvas's bottom left, over the bodies and under what the tab's mode
    // draws, and the view label beside it over that (the prototype's triad is its overlay's
    // first, its label HTML above the overlay), both of the camera this frame draws.
    let painter = ui.painter_at(rect);
    if let Some(triad) = crate::triad::triad(&view.camera, rect) {
        painter.extend(triad.shapes(&painter));
    }
    let label = crate::triad::view_label(view.labelled_camera(), rect);
    // Over the bodies, under the ViewCube and the bar, in the pane's own layer: what the
    // tab's mode draws (a sketch, a selection's marks). It takes no input; the cube's and
    // the bar's, registered after, come first. A camera they change is drawn next frame.
    if let Some(mut frame) = view.frame.take() {
        overlaid(tab, sessions, &painter, &frame);
        // Handed on once: what reads the frame after reads no event again.
        frame.left.clear();
        frame.secondary.clear();
        tab.view.frame = Some(frame);
    }
    // The canvas's hover now over what another feature made than the timeline rang (or over
    // nothing now, or something now): the timeline, drawn before the canvas, rings it in
    // the next frame, asked for at once.
    if tab.selection().hovered_feature() != tab.view.ringed_feature.as_ref() {
        ui.ctx().request_repaint();
    }
    let (shape, line) = label.shape(&painter);
    painter.add(shape);
    // In the accessibility tree as a label, taking no click or drag of the canvas's.
    let label_response = ui.interact(line, ui.id().with("view_label"), Sense::hover());
    label_response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &label.words));
    view_cube(ui, rect, &mut tab.view, size_px);
    // The bar's Look At, as Ctrl+8's: square to the face selected, or to the sketch.
    if navigation_bar(ui, rect, &mut tab.view, size_px) {
        tab.look_at();
    }
    // A transition under way, or begun this frame (by an ask, the ViewCube, or the bar's
    // Fit): the next frame moves it on.
    if tab.view.transition.is_some() {
        ui.ctx().request_repaint();
    }
    // An empty part's hint, above the bar: Create Sketch, as the toolbar's. And Create
    // Sketch's pick's, at the top, with its Cancel.
    crate::canvas_hint::empty_part(ui, rect, tab);
    crate::canvas_hint::plane_pick(ui, rect, tab);
}

/// The faces `feature` made in `bodies`, as each face's name says ([`Name::feature`]), in
/// every body that holds one, ascending; or, `whole`, a body's every face where it's the
/// body `feature` made (a body selected by its browser row, the prototype's `maker: '*'`).
/// One rule for a feature hovered in the timeline and one selected.
///
/// [`Name::feature`]: silico_core::naming::Name::feature
pub fn faces_of(
    feature: &FeatureId,
    bodies: &BTreeMap<u32, ViewBody>,
    whole: bool,
) -> Vec<(u32, Element)> {
    bodies
        .iter()
        .flat_map(|(number, body)| {
            let whole = whole && &body.feature == feature;
            body.faces
                .iter()
                .zip(1u32..)
                .filter(move |(face, _)| whole || face.name.feature() == feature)
                .map(|(_, n)| (*number, Element::Face(n)))
        })
        .collect()
}

/// The tint of the faces a feature hovered in the timeline made, from the theme, as plain
/// values for the renderer: the prototype's `MAKER` (DESIGN.md's Blue.800) at 0.4.
pub fn feature_tint() -> Tint {
    let [r, g, b, _] = theme::VIEWPORT_FEATURE_HOVER_TINT;
    Tint {
        colour: [r, g, b],
        amount: theme::VIEWPORT_FEATURE_HOVER_TINT_AMOUNT,
    }
}

/// Which of `makers`, the faces tinted as their maker's ([`View::maker_faces`]), are tinted:
/// those `drawn`, but any in `highlights` or `hovered` (the selection's, and the hover's or a
/// candidate's), which keep their own look, as the prototype's `applyHighlight` gives a face
/// the selection's look, else the hover's, else the maker's tint. Ascending, each once.
pub fn tinted_faces(
    makers: Vec<(u32, Element)>,
    highlights: &[(u32, Element)],
    hovered: &[(u32, Element)],
    drawn: impl Fn(&(u32, Element)) -> bool,
) -> Vec<(u32, Element)> {
    let kept: BTreeSet<&(u32, Element)> = highlights.iter().chain(hovered).collect();
    makers
        .into_iter()
        .filter(|face| drawn(face) && !kept.contains(face))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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

/// A face of the ViewCube: its named view, its outward normal, and the way its label's
/// words run across it and the way their tops point, each a unit axis of the cube.
#[derive(Debug, Clone, Copy)]
struct CubeFace {
    view: NamedView,
    normal: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
}

/// The ViewCube's faces, each label lying as the prototype's textures do (viewport.js's
/// `buildCube`: three.js's `BoxGeometry` turned a quarter about x, so its +y face is the
/// top and its +z the front), read from outside the cube and upright from the face's own
/// view: the sides' words z up, the top's tops toward the back. The prototype's bottom view
/// looks with −y up, and its bottom's words read upright there, their tops toward the
/// front; the app's (`NamedView::up`) looks with +y up, so the bottom's tops are toward the
/// back here, or Bottom would show its own word upside down. Each face's `right × up` is
/// its normal.
const FACES: [CubeFace; 6] = [
    CubeFace {
        view: NamedView::Front,
        normal: [0.0, -1.0, 0.0],
        right: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    },
    CubeFace {
        view: NamedView::Back,
        normal: [0.0, 1.0, 0.0],
        right: [-1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    },
    CubeFace {
        view: NamedView::Left,
        normal: [-1.0, 0.0, 0.0],
        right: [0.0, -1.0, 0.0],
        up: [0.0, 0.0, 1.0],
    },
    CubeFace {
        view: NamedView::Right,
        normal: [1.0, 0.0, 0.0],
        right: [0.0, 1.0, 0.0],
        up: [0.0, 0.0, 1.0],
    },
    CubeFace {
        view: NamedView::Top,
        normal: [0.0, 0.0, 1.0],
        right: [1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
    },
    CubeFace {
        view: NamedView::Bottom,
        normal: [0.0, 0.0, -1.0],
        right: [-1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
    },
];

/// How far a face must face the view to be drawn: an edge-on face isn't.
const CUBE_FACE_SHOWN: f64 = 1e-9;

/// How far a face must face the view for its label to be drawn and its click taken.
const CUBE_FACE_LABELLED: f64 = 0.25;

/// How far a corner's view, or an edge's, must face the view for its click to be taken, and
/// the side of the square about it that takes it.
const CUBE_CORNER_SHOWN: f64 = 0.1;
const CUBE_CORNER_SIDE: f32 = 12.0;

/// The cube's half side as a part of the ViewCube's side: the prototype's cube camera spans
/// 3.5 of the cube's units across the square it's drawn in, the cube 2 of them
/// (viewport.js's `buildCube`: `OrthographicCamera(-1.75, 1.75, 1.75, -1.75, …)` and
/// `BoxGeometry(2, 2, 2)`).
const CUBE_HALF_SIDE: f64 = 1.0 / 3.5;

/// The cube's axis stubs (viewport.js's `buildCube`, `ax`): from just outside its bottom
/// front left corner, 1.02 half sides from its centre on each axis, 1.4 half sides along x,
/// y, and z, each in its axis's colour, in the document's axes as the cube's faces are.
const CUBE_STUB_FROM: [f64; 3] = [-1.02, -1.02, -1.02];
const CUBE_STUB_LENGTH: f64 = 1.4;

/// Narrows `s`, an interval of s, to where c + m·s ≤ 0: false if nothing's left.
fn keep_where_not_above_zero(s: &mut [f64; 2], c: f64, m: f64) -> bool {
    if m > 0.0 {
        s[1] = s[1].min(-c / m);
    } else if m < 0.0 {
        s[0] = s[0].max(-c / m);
    } else if c > 0.0 {
        return false;
    }
    s[0] <= s[1]
}

/// The part of the segment from `from` to `from + along` (s from 0 to 1 along it) the cube
/// hides from a view looking along −`back`, as the cube is drawn, orthographic: where the
/// ray from the point toward the eye, along `back`, meets the cube (its points at most 1
/// from its centre on each axis). The cube is convex, so that's one interval of s, or none:
/// the s for which some t ≥ 0 puts `from + s·along + t·back` in the cube, each axis
/// bounding t from below and above by a line in s (or s itself, an axis the view is square
/// to), and every lower bound at most every upper one.
fn hidden_by_cube(from: [f64; 3], along: [f64; 3], back: [f64; 3]) -> Option<[f64; 2]> {
    let mut s = [0.0, 1.0];
    // t ≥ c + m·s, and t ≤ c + m·s, each as (c, m): t ≥ 0 first.
    let mut lower = vec![(0.0, 0.0)];
    let mut upper = Vec::new();
    for k in 0..3 {
        let (a, d, b) = (from[k], along[k], back[k]);
        if b == 0.0 {
            // −1 ≤ a + d·s ≤ 1.
            if !keep_where_not_above_zero(&mut s, a - 1.0, d)
                || !keep_where_not_above_zero(&mut s, -1.0 - a, -d)
            {
                return None;
            }
        } else {
            // a + d·s + t·b from −1 to 1: t between (−1 − a − d·s)/b and (1 − a − d·s)/b,
            // the lesser below, two lines in s of one slope.
            let slope = -d / b;
            let (one, other) = ((-1.0 - a) / b, (1.0 - a) / b);
            lower.push((one.min(other), slope));
            upper.push((one.max(other), slope));
        }
    }
    if upper.is_empty() {
        // No way to look along: nothing's hidden.
        return None;
    }
    for &(lc, lm) in &lower {
        for &(uc, um) in &upper {
            if !keep_where_not_above_zero(&mut s, lc - uc, lm - um) {
                return None;
            }
        }
    }
    Some(s)
}

/// The parts of the segment from `from` to `from + along` the cube doesn't hide from a view
/// looking along −`back` ([`hidden_by_cube`]), each by its s from 0 to 1: the whole, or what's
/// before and after the part it hides, each only if it's of some length.
fn seen_past_cube(from: [f64; 3], along: [f64; 3], back: [f64; 3]) -> Vec<[f64; 2]> {
    match hidden_by_cube(from, along, back) {
        None => vec![[0.0, 1.0]],
        Some([start, end]) => [[0.0, start], [end, 1.0]]
            .into_iter()
            .filter(|[a, b]| a < b)
            .collect(),
    }
}

/// A face's fill (viewport.js's `buildCube`).
fn cube_face_fill(view: NamedView) -> egui::Color32 {
    match view {
        NamedView::Top => theme::VIEW_CUBE_TOP_FILL,
        NamedView::Bottom => theme::VIEW_CUBE_BOTTOM_FILL,
        NamedView::Front => theme::VIEW_CUBE_FRONT_FILL,
        NamedView::Back => theme::VIEW_CUBE_BACK_FILL,
        _ => theme::VIEW_CUBE_SIDE_FILL,
    }
}

/// An edge of the ViewCube: its two ends, corners of the cube (each coordinate ±1), and the
/// outward normals of the two faces it joins.
#[derive(Debug, Clone, Copy)]
struct CubeEdge {
    ends: [[f64; 3]; 2],
    faces: [[f64; 3]; 2],
}

/// The cube's twelve edges: for each axis, the four along it, from −1 to 1 on it.
fn cube_edges() -> impl Iterator<Item = CubeEdge> {
    (0..3).flat_map(|k| {
        let (a, b) = ((k + 1) % 3, (k + 2) % 3);
        [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)]
            .into_iter()
            .map(move |(sa, sb)| {
                let mut start = [0.0; 3];
                start[a] = sa;
                start[b] = sb;
                let mut end = start;
                start[k] = -1.0;
                end[k] = 1.0;
                let (mut one, mut other) = ([0.0; 3], [0.0; 3]);
                one[a] = sa;
                other[b] = sb;
                CubeEdge {
                    ends: [start, end],
                    faces: [one, other],
                }
            })
    })
}

/// The named views the ViewCube's edges and corners give, the edges' first: each between
/// two of its faces or three ([`NamedView::faces`]), in [`NAMED_VIEWS`]' order within each.
fn cube_points() -> impl Iterator<Item = NamedView> {
    let edges = NAMED_VIEWS.into_iter().filter(|view| view.faces() == 2);
    let corners = NAMED_VIEWS.into_iter().filter(|view| view.faces() == 3);
    edges.chain(corners)
}

/// Where a point of the cube's face lands: the face's centre on screen, and how far one of
/// the face's units (its half side) across it and down it move on screen.
#[derive(Debug, Clone, Copy)]
struct OnFace {
    centre: Pos2,
    across: Vec2,
    down: Vec2,
}

impl OnFace {
    /// The point `u` across the face and `v` down it, each in its half sides (its corners
    /// at ±1): an orthographic view maps the face affinely, so a square on it is a
    /// parallelogram on screen.
    fn at(&self, u: f32, v: f32) -> Pos2 {
        self.centre + self.across * u + self.down * v
    }
}

/// A face's label, waiting for the pass's end ([`CubeLabels`]): its words laid out at the
/// size they have face-on, where its mesh goes in the paint list, and how it lies on its
/// face.
struct CubeLabel {
    galley: Arc<egui::Galley>,
    layer: egui::LayerId,
    clip: Rect,
    opacity: f32,
    at: egui::layers::ShapeIdx,
    face: OnFace,
    /// Points of the galley a half side of the face holds: the cube's scale.
    per_unit: f32,
}

/// The ViewCube's labels, each drawn on its face at the pass's end: a label is its galley's
/// glyphs as a mesh, each vertex mapped onto the face, so it foreshortens with the face and
/// stays inside it; the mesh's texture coordinates are the font atlas's, which grows while
/// a pass lays out text, so they're taken from the atlas only once the pass has laid out
/// everything. Each label holds the place its face kept for it in the paint list, so it's
/// drawn over its face and under the cube's edges.
#[derive(Default)]
struct CubeLabels {
    waiting: Vec<CubeLabel>,
}

impl egui::Plugin for CubeLabels {
    fn debug_name(&self) -> &'static str {
        "ViewCube labels"
    }

    fn on_begin_pass(&mut self, _ui: &mut Ui) {
        // Those of a pass that never ended are another pass's places.
        self.waiting.clear();
    }

    fn on_end_pass(&mut self, ui: &mut Ui) {
        for label in self.waiting.drain(..) {
            let mesh = label_mesh(ui.ctx(), &label);
            let mut painter = ui
                .ctx()
                .layer_painter(label.layer)
                .with_clip_rect(label.clip);
            painter.set_opacity(label.opacity);
            painter.set(label.at, egui::Shape::mesh(mesh));
        }
    }
}

/// `label`'s glyphs as a mesh on its face: the galley tessellated about its centre, at the
/// font atlas's size now, each vertex's offset from that centre, in face units, taken
/// across and down the face from the label's middle ([`theme::VIEW_CUBE_LABEL_DROP`] below
/// the face's).
fn label_mesh(ctx: &egui::Context, label: &CubeLabel) -> egui::Mesh {
    let mut options = ctx.tessellation_options(|options| *options);
    // Placed on the face after, so not snapped to pixels here.
    options.round_text_to_pixels = false;
    let atlas = ctx.fonts(|fonts| fonts.font_image_size());
    let mut tessellator =
        egui::epaint::Tessellator::new(ctx.pixels_per_point(), options, atlas, Vec::new());
    let middle = label.galley.rect.center().to_vec2();
    let text = egui::epaint::TextShape::new(
        Pos2::ZERO - middle,
        Arc::clone(&label.galley),
        theme::VIEW_CUBE_LABEL,
    );
    let mut mesh = egui::Mesh::default();
    tessellator.tessellate_text(&text, &mut mesh);
    let drop = 2.0 * theme::VIEW_CUBE_LABEL_DROP;
    for vertex in &mut mesh.vertices {
        let u = vertex.pos.x / label.per_unit;
        let v = drop + vertex.pos.y / label.per_unit;
        vertex.pos = label.face.at(u, v);
    }
    mesh
}

/// A face of the ViewCube as the prototype's texture paints it (viewport.js's `label`): the
/// border's grey over the whole face, and `fill` inside the border, neither stroked: the
/// cube's edges are drawn once, over them.
fn paint_cube_face(painter: &egui::Painter, on: OnFace, fill: egui::Color32) {
    let corners = |half: f32| {
        vec![
            on.at(half, half),
            on.at(half, -half),
            on.at(-half, -half),
            on.at(-half, half),
        ]
    };
    let border = egui::Shape::convex_polygon(corners(1.0), theme::VIEW_CUBE_BORDER, Stroke::NONE);
    painter.add(border);
    let inside = 1.0 - 2.0 * theme::VIEW_CUBE_BORDER_WIDTH;
    painter.add(egui::Shape::convex_polygon(
        corners(inside),
        fill,
        Stroke::NONE,
    ));
}

/// Keeps `words`' place in the paint list, over the face `on`, for its label, drawn there at
/// the pass's end ([`CubeLabels`]): Inter Medium at [`theme::VIEW_CUBE_LABEL_SIZE`] of the
/// face's side face-on, `per_unit` points to its half side. The first pass that labels a
/// face registers the labels' plugin, and its labels are drawn from the next, asked for at
/// once: a pass's plugins are those it began with.
fn label_cube_face(painter: &egui::Painter, on: OnFace, per_unit: f32, words: &str) {
    if !painter.is_visible() {
        return;
    }
    let size = theme::VIEW_CUBE_LABEL_SIZE * 2.0 * per_unit;
    let galley = painter.layout_no_wrap(
        words.to_owned(),
        egui::FontId::proportional(size),
        theme::VIEW_CUBE_LABEL,
    );
    let label = CubeLabel {
        galley,
        layer: painter.layer_id(),
        clip: painter.clip_rect(),
        opacity: painter.opacity(),
        at: painter.add(egui::Shape::Noop),
        face: on,
        per_unit,
    };
    let ctx = painter.ctx();
    if ctx
        .with_plugin(|labels: &mut CubeLabels| labels.waiting.push(label))
        .is_none()
    {
        ctx.add_plugin(CubeLabels::default());
        ctx.request_repaint();
    }
}

/// A clickable part of the ViewCube, labelled for the accessibility tree.
fn cube_part(ui: &mut Ui, rect: Rect, label: &str) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(("viewcube", label)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    response.on_hover_text(label)
}

/// The ViewCube's menu's name: its ▾'s tooltip's title (app.js's `cubeMenu` button), and
/// what a screen reader finds the ▾ and the menu by.
pub const CUBE_MENU: &str = "View cube options";

/// The display settings' name: its button's tooltip's title (app.js's `displayMenu`
/// button), and what a screen reader finds the button and its menu by.
pub const DISPLAY_SETTINGS: &str = "Display settings";

/// The one visual style the renderer draws (the prototype's display menu's first, its
/// others left out until the renderer draws them).
pub const SHADED_WITH_VISIBLE_EDGES: &str = "Shaded with visible edges";

/// What the ViewCube's menu asks: a view, or the projection.
#[derive(Debug, Clone, Copy)]
enum CubeAct {
    View(ViewAsk),
    Perspective(bool),
}

/// The ViewCube's menu (interact.js's `cubeMenu`; fusion.md, *ViewCube and Navigation Bar*:
/// Go Home and *Camera*): Home view, with its icon and its key; under *Views*, the named
/// views the registry's keys give (Front to Bottom, and Isometric), by its words and with
/// its keys; and under *Camera*, the projection, the view's marked. What was chosen.
fn cube_menu(ui: &mut Ui, perspective: bool) -> Option<CubeAct> {
    let ctx = ui.ctx().clone();
    let views: Vec<(Command, ViewAsk, Vec<String>)> = VIEW_COMMANDS
        .into_iter()
        .filter(|(command, _)| *command != Command::Fit)
        .map(|(command, ask)| (command, ask, command.caps(&ctx)))
        .collect();
    let mut lines = Vec::new();
    for (command, ask, caps) in &views {
        let home = *command == Command::Home;
        let item = menu::Item {
            icon: home.then_some(Icon::Home),
            label: command.label(),
            sub: None,
            caps,
            off: None,
            danger: false,
            lit: false,
            checked: None,
        };
        lines.push(menu::Line::Item(item, CubeAct::View(*ask)));
        if home {
            lines.push(menu::Line::Separator);
            lines.push(menu::Line::Head("Views"));
        }
    }
    lines.push(menu::Line::Separator);
    lines.push(menu::Line::Head("Camera"));
    for (word, wanted) in [
        (crate::words::ORTHOGRAPHIC, false),
        (crate::words::PERSPECTIVE, true),
    ] {
        let item = menu::Item {
            icon: None,
            label: word,
            sub: None,
            caps: &[],
            off: None,
            danger: false,
            lit: false,
            checked: Some(Checked::Choice(perspective == wanted)),
        };
        lines.push(menu::Line::Item(item, CubeAct::Perspective(wanted)));
    }
    menu::lines(ui, CUBE_MENU, &lines)
}

/// What the display settings' menu asks: the visual style (the one there is), the
/// projection, or the ground grid shown or hidden.
#[derive(Debug, Clone, Copy)]
enum DisplayAct {
    Style,
    Perspective(bool),
    Grid,
}

/// The display settings' menu (interact.js's `displayMenu`; fusion.md, *Display Settings*),
/// of what the app draws: under *Visual style*, the one style the renderer draws, marked
/// (the prototype's others, Shaded, Wireframe, and Hidden edges, left out until the renderer
/// draws them); under *Camera*, the projection, the view's marked; and the ground grid,
/// marked while it's shown. What was chosen.
fn display_menu(ui: &mut Ui, perspective: bool, grid: bool) -> Option<DisplayAct> {
    let item = |icon: Option<Icon>, label: &'static str, checked: Checked| menu::Item {
        icon,
        label,
        sub: None,
        caps: &[],
        off: None,
        danger: false,
        lit: false,
        checked: Some(checked),
    };
    let lines = [
        menu::Line::Head("Visual style"),
        menu::Line::Item(
            item(None, SHADED_WITH_VISIBLE_EDGES, Checked::Choice(true)),
            DisplayAct::Style,
        ),
        menu::Line::Separator,
        menu::Line::Head("Camera"),
        menu::Line::Item(
            item(
                Some(Icon::Ortho),
                crate::words::ORTHOGRAPHIC,
                Checked::Choice(!perspective),
            ),
            DisplayAct::Perspective(false),
        ),
        menu::Line::Item(
            item(
                Some(Icon::Persp),
                crate::words::PERSPECTIVE,
                Checked::Choice(perspective),
            ),
            DisplayAct::Perspective(true),
        ),
        menu::Line::Separator,
        menu::Line::Item(
            item(Some(Icon::Grid), "Ground grid", Checked::Toggle(grid)),
            DisplayAct::Grid,
        ),
    ];
    menu::lines(ui, DISPLAY_SETTINGS, &lines)
}

/// Where the ViewCube is drawn in `viewport`: its margin in from the canvas's top and right.
pub fn view_cube_rect(viewport: Rect) -> Rect {
    Rect::from_min_size(
        egui::pos2(
            viewport.right() - theme::VIEW_CUBE_SIDE - theme::VIEW_CUBE_MARGIN,
            viewport.top() + theme::VIEW_CUBE_MARGIN,
        ),
        Vec2::splat(theme::VIEW_CUBE_SIDE),
    )
}

/// Where the ViewCube's Home view and ▾ are drawn beside the cube at `cube`: bar buttons
/// (one icon each) in a column at its left, its right [`theme::VIEW_CUBE_BUTTONS_GAP`] left
/// of the cube, Home's top level with the cube's, the ▾ under it.
fn view_cube_buttons(cube: Rect) -> [Rect; 2] {
    let style = bar::ButtonStyle::VIEW_CUBE;
    let size = Vec2::new(style.width(bar::content_width(1, None)), style.height);
    let home = Rect::from_min_size(
        egui::pos2(
            cube.left() - theme::VIEW_CUBE_BUTTONS_GAP - size.x,
            cube.top(),
        ),
        size,
    );
    let options = home.translate(Vec2::new(0.0, size.y + theme::VIEW_CUBE_BUTTONS_SPACING));
    [home, options]
}

/// The ViewCube, at the canvas's top right (section 8; fusion.md, *ViewCube and Navigation
/// Bar*): the cube as the camera sees it, drawn as the prototype's (viewport.js's
/// `buildCube` and `label`): each face it faces filled in its own grey inside a border, its
/// label in capitals lying on it, each edge of a face drawn once over them, and the axis
/// stubs at its bottom front left corner, but where the cube is in front of them. A face's
/// click gives its orthographic view, an edge's the view between its two faces', a corner's
/// its isometric one, and a left drag orbits. Beside it, Home view gives the home view, and
/// the ▾ opens its menu (Home view, the named views, and the projection, which the views it
/// gives keep), as a right click on the cube does.
fn view_cube(ui: &mut Ui, viewport: Rect, view: &mut View, size_px: [u32; 2]) {
    let side = theme::VIEW_CUBE_SIDE;
    let rect = view_cube_rect(viewport);
    let [home, options] = view_cube_buttons(rect);
    let Ok(frame) = view.camera.frame() else {
        return;
    };
    ui.scope_builder(UiBuilder::new().id_salt("viewcube"), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Group);
            node.set_label("ViewCube");
            // A group that senses nothing gets no bounds of egui's: the cube and its buttons.
            node.set_bounds(crate::panes::bounds(rect.union(home).union(options)));
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
        let scale = f64::from(side) * CUBE_HALF_SIDE;
        let offset = |v: [f64; 3]| navigate::screen_offset(&frame, v, scale);
        let at = |v: [f64; 3]| rect.center() + offset(v);
        let facing = |normal: [f64; 3]| silico_protocol::vec3::dot(normal, frame.back);
        let painter = ui.painter_at(rect);
        let mut chosen = None;
        for face in FACES {
            let faced = facing(face.normal);
            if faced <= CUBE_FACE_SHOWN {
                continue;
            }
            let on = OnFace {
                centre: at(face.normal),
                across: offset(face.right),
                down: offset(face.up.map(|c| -c)),
            };
            paint_cube_face(&painter, on, cube_face_fill(face.view));
            if faced > CUBE_FACE_LABELLED {
                let label = crate::words::view_words(face.view);
                label_cube_face(&painter, on, scale as f32, &label.to_uppercase());
                let part = Rect::from_center_size(on.centre, Vec2::splat(side * 0.3));
                let response = cube_part(ui, part, &label);
                if response.clicked() {
                    chosen = Some(ViewAsk::Named(face.view));
                }
                parts.push(response);
            }
        }
        // Each edge of a face drawn, once, over the faces: so one shared by two is no wider.
        for edge in cube_edges() {
            if edge
                .faces
                .iter()
                .any(|normal| facing(*normal) > CUBE_FACE_SHOWN)
            {
                let [start, end] = edge.ends;
                painter.line_segment([at(start), at(end)], theme::VIEW_CUBE_EDGE);
            }
        }
        // The axis stubs at its corner, over its edges, each but where the cube is in front
        // of it (the prototype's are depth tested), and none seen end on, a point.
        for (k, colour) in [theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z]
            .into_iter()
            .enumerate()
        {
            let mut along = [0.0; 3];
            along[k] = CUBE_STUB_LENGTH;
            let point = |s: f64| {
                let mut p = CUBE_STUB_FROM;
                p[k] += s * along[k];
                at(p)
            };
            let stroke = Stroke::new(theme::VIEW_CUBE_STUB_WIDTH, theme::color32(colour));
            for [a, b] in seen_past_cube(CUBE_STUB_FROM, along, frame.back) {
                let ends = [point(a), point(b)];
                if ends[0] != ends[1] {
                    painter.line_segment(ends, stroke);
                }
            }
        }
        // Each edge's view and each corner's whose direction faces the view more than a
        // tenth, clickable at its point on the cube (an edge's middle, a corner), by its
        // words: the edges first, so a corner keeps its own square where they meet.
        for named in cube_points() {
            if facing(named.direction()) <= CUBE_CORNER_SHOWN {
                continue;
            }
            let point = Rect::from_center_size(at(named.axes()), Vec2::splat(CUBE_CORNER_SIDE));
            let response = cube_part(ui, point, &crate::words::view_words(named));
            if response.clicked() {
                chosen = Some(ViewAsk::Named(named));
            }
            parts.push(response);
        }
        // Home view and the ▾, in their column at its left.
        let ctx = ui.ctx().clone();
        let style = bar::ButtonStyle::VIEW_CUBE;
        let label = Command::Home.label();
        let home = ui.put(home, |ui: &mut Ui| {
            bar::styled_button(ui, &[Icon::Home], None, label, true, None, style)
        });
        if bar::tooltip(home, label, Command::Home.shown(&ctx), None).clicked() {
            chosen = Some(ViewAsk::Named(HOME));
        }
        let options = ui.put(options, |ui: &mut Ui| {
            bar::styled_button(ui, &[Icon::Down], None, CUBE_MENU, true, None, style)
        });
        ctx.accesskit_node_builder(options.id, |node| {
            node.set_has_popup(egui::accesskit::HasPopup::Menu);
        });
        let options = bar::tooltip(
            options,
            CUBE_MENU,
            None,
            Some("Orthographic or perspective"),
        );
        // Its menu: under the ▾, or at a right click anywhere on the cube; the ▾ again
        // closes it.
        let menu_id = ui.id().with("menu");
        let opened_at = if options.clicked() {
            if egui::Popup::is_id_open(&ctx, menu_id) {
                egui::Popup::close_id(&ctx, menu_id);
                None
            } else {
                Some(options.rect.left_bottom() + theme::VIEW_CUBE_MENU_FROM_BUTTON)
            }
        } else if parts.iter().any(egui::Response::secondary_clicked) {
            ctx.pointer_interact_pos()
                .map(|at| at + theme::VIEW_CUBE_MENU_FROM_CLICK)
        } else {
            None
        };
        if let Some(at) = opened_at {
            ctx.data_mut(|data| data.insert_temp(menu_id, at));
            egui::Popup::open_id(&ctx, menu_id);
        }
        let anchor = ctx
            .data(|data| data.get_temp::<Pos2>(menu_id))
            .filter(|_| egui::Popup::is_id_open(&ctx, menu_id));
        if let Some(at) = anchor {
            let popup = egui::Popup::new(menu_id, ctx.clone(), at, ui.layer_id()).open_memory(None);
            let perspective = view.perspective;
            let asked = bar::as_menu(popup)
                .show(|ui| cube_menu(ui, perspective))
                .and_then(|shown| shown.inner);
            match asked {
                Some(CubeAct::View(ask)) => chosen = Some(ask),
                Some(CubeAct::Perspective(wanted)) => view.project(wanted),
                None => {}
            }
        }
        if let Some(ask) = chosen {
            view.give(ask, size_px, Motion::of(&ctx));
        }
    });
}

/// The navigation bar's tools, each the left drag's while it's chosen: what it does, its
/// icon, its words (the prototype's `data-tip`), the registry's command whose keys its
/// tooltip names (`data-tipk`), and its tooltip's line (`data-tipd`; Orbit's but for its
/// last sentence, "Alt-drag also orbits in this prototype", which the app doesn't).
const TOOLS: [(Navigation, Icon, &str, Command, Option<&str>); 3] = [
    (
        Navigation::Orbit,
        Icon::Orbit,
        "Orbit",
        Command::Orbit,
        Some("Or click here, then drag with the left button. Esc returns to Select."),
    ),
    (Navigation::Pan, Icon::Pan, "Pan", Command::Pan, None),
    (
        Navigation::Zoom,
        Icon::Zoom,
        "Zoom",
        Command::Zoom,
        Some("The wheel zooms toward the pointer"),
    ),
];

/// Where the navigation bar is drawn in `viewport`: its margin above the canvas's bottom,
/// about its middle.
fn navigation_bar_rect(viewport: Rect) -> Rect {
    let size = Vec2::new(theme::NAVIGATION_BAR_WIDTH, theme::NAVIGATION_BAR_HEIGHT);
    Rect::from_center_size(
        egui::pos2(
            viewport.center().x,
            viewport.bottom() - size.y / 2.0 - theme::NAVIGATION_BAR_MARGIN,
        ),
        size,
    )
}

/// The navigation bar, floating at the canvas's bottom centre (section 8; app.js's
/// `.nav-bar`), its buttons the prototype's icons, each with its tooltip: Orbit, Pan, and
/// Zoom, each the left drag's tool while chosen (a second click, or Esc, lets it go); Fit;
/// Look At; a separator; Display settings, whose menu opens above it; and Grid, which shows
/// and hides the ground grid. A press anywhere on it, its padding too, is its own, never the
/// canvas's. Whether Look At was clicked, which the tab does as Ctrl+8 does
/// ([`Tab::look_at`]): it needs the tab's selection, or its sketch.
fn navigation_bar(ui: &mut Ui, viewport: Rect, view: &mut View, size_px: [u32; 2]) -> bool {
    let rect = navigation_bar_rect(viewport);
    let mut look_at = false;
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("navigation_bar")
            .max_rect(rect)
            // Under its buttons, over the canvas: its padding and border absorb a press.
            .sense(Sense::click_and_drag()),
        |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Toolbar);
                node.set_label("Navigation bar");
            });
            let corner = theme::NORMAL_CORNER_RADIUS;
            let painter = ui.painter();
            painter.add(theme::NAVIGATION_BAR_SHADOW.as_shape(rect, corner));
            painter.rect(
                rect,
                corner,
                theme::NAVIGATION_BAR_BG,
                Stroke::new(theme::BORDER_WIDTH, theme::NAVIGATION_BAR_BORDER),
                StrokeKind::Inside,
            );
            let inner = rect.shrink(theme::BORDER_WIDTH + theme::NAVIGATION_BAR_PADDING);
            let row = UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::left_to_right(Align::Center));
            ui.scope_builder(row, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(theme::NAVIGATION_BAR_GAP, 0.0);
                let ctx = ui.ctx().clone();
                let style = bar::ButtonStyle::NAVIGATION_BAR;
                for (tool, icon, word, command, said) in TOOLS {
                    let on = view.tool == Some(tool);
                    let button = bar::styled_button(ui, &[icon], None, word, true, Some(on), style);
                    if bar::tooltip(button, word, command.shown(&ctx), said).clicked() {
                        view.tool = (!on).then_some(tool);
                    }
                }
                let word = Command::Fit.label();
                let fit = bar::styled_button(ui, &[Icon::Fit], None, word, true, None, style);
                if bar::tooltip(fit, word, Command::Fit.shown(&ctx), None).clicked() {
                    view.fit(size_px, Motion::of(&ctx));
                }
                let word = Command::LookAt.label();
                let look = bar::styled_button(ui, &[Icon::LookAt], None, word, true, None, style);
                let said = Some("Square to the selected face");
                look_at = bar::tooltip(look, word, Command::LookAt.shown(&ctx), said).clicked();
                bar::separator_of(
                    ui,
                    theme::NAVIGATION_BAR_SEPARATOR_HEIGHT,
                    theme::NAVIGATION_BAR_SEPARATOR_MARGIN,
                );
                let icons = [Icon::Display, Icon::Down];
                let display =
                    bar::styled_button(ui, &icons, None, DISPLAY_SETTINGS, true, None, style);
                ctx.accesskit_node_builder(display.id, |node| {
                    node.set_has_popup(egui::accesskit::HasPopup::Menu);
                });
                let said = Some("Visual style, camera, grid");
                let display = bar::tooltip(display, DISPLAY_SETTINGS, None, said);
                let (perspective, grid) = (view.perspective, view.grid);
                let popup = egui::Popup::menu(&display)
                    .align(egui::RectAlign::TOP_START)
                    .gap(theme::NAVIGATION_BAR_MENU_GAP);
                let asked = bar::as_menu(popup)
                    .show(|ui| display_menu(ui, perspective, grid))
                    .and_then(|shown| shown.inner);
                match asked {
                    Some(DisplayAct::Perspective(wanted)) => view.project(wanted),
                    Some(DisplayAct::Grid) => view.grid = !view.grid,
                    Some(DisplayAct::Style) | None => {}
                }
                // The canvas was painted before the bar: the next frame, which egui draws
                // after any frame with input, shows the change.
                let shown = Some(view.grid);
                let grid = bar::styled_button(ui, &[Icon::Grid], None, "Grid", true, shown, style);
                if bar::tooltip(grid, "Grid", None, None).clicked() {
                    view.grid = !view.grid;
                }
            });
            // Its whole place its own, padding and border too (egui senses a scope by the
            // rect it ended with), and its bounds that place: egui gives a scope that senses
            // the bounds it starts with, none (a size of NaN), until it ends.
            ui.expand_to_include_rect(rect);
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_bounds(crate::panes::bounds(rect));
            });
        },
    );
    look_at
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
                vertex_points: vec![],
            }),
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
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
            vertices: Vec::new(),
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
    fn the_view_cube_sits_its_margin_in_from_the_canvass_top_right_its_buttons_at_its_left() {
        // A canvas from (100, 50) to (900, 650): the cube's 92-point square (the prototype's
        // `CUBE`) 16 points (`CUBE_M`) in from its right edge and its top, 900 − 16 − 92 =
        // 792 to 900 − 16 = 884 across, 50 + 16 = 66 to 66 + 92 = 158 down. Home view and the
        // ▾ (app.js: `right: 16 + 92 + 4`, `top: 16px`, a column 2 apart), each a bar button
        // of one 14-point icon, 5 of padding each side (24 wide, over `--btn`'s 22) and 22
        // high: from 792 − 4 − 24 = 764 to 788 across; Home 66 to 88 down, the ▾ 90 to 112.
        let viewport = Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(900.0, 650.0));
        let cube = view_cube_rect(viewport);
        assert_eq!(
            cube,
            Rect::from_min_max(egui::pos2(792.0, 66.0), egui::pos2(884.0, 158.0))
        );
        assert_eq!(
            view_cube_buttons(cube),
            [
                Rect::from_min_max(egui::pos2(764.0, 66.0), egui::pos2(788.0, 88.0)),
                Rect::from_min_max(egui::pos2(764.0, 90.0), egui::pos2(788.0, 112.0)),
            ]
        );
    }

    #[test]
    fn the_navigation_bar_sits_its_margin_above_the_canvass_bottom_about_its_middle() {
        // The canvas from (100, 50) to (900, 650): 220 by 32 (`.nav-bar`), its middle at x
        // 500 and its bottom 12 above 650, so from (390, 606) to (610, 638).
        let viewport = Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(900.0, 650.0));
        assert_eq!(
            navigation_bar_rect(viewport),
            Rect::from_min_max(egui::pos2(390.0, 606.0), egui::pos2(610.0, 638.0))
        );
    }

    #[test]
    fn every_view_the_cube_offers_is_a_named_view_and_its_click_gives_that_views_camera() {
        use silico_protocol::vec3::{dot, sub};
        // The cube's faces, its edges' middles and its corners, Home, and its menu's views:
        // each a `NamedView` asked for by itself, which parses back from its name, and
        // together all 26, each face, edge, and corner once.
        let mut offered: Vec<NamedView> = FACES.iter().map(|face| face.view).collect();
        offered.extend(cube_points());
        let mut every = offered.clone();
        for (_, ask) in VIEW_COMMANDS {
            if let ViewAsk::Named(view) = ask {
                every.push(view);
            }
        }
        for view in &every {
            assert_eq!(NamedView::named(view.name()), Some(*view), "{view:?}");
        }
        assert_eq!(offered.len(), 26);
        let distinct: BTreeSet<NamedView> = offered.iter().copied().collect();
        assert_eq!(distinct, NAMED_VIEWS.into_iter().collect::<BTreeSet<_>>());
        // The menu: Home and Isometric the home corner, the rest the faces.
        let menu: Vec<NamedView> = every[26..].to_vec();
        assert_eq!(
            menu,
            [
                NamedView::TopFrontRight,
                NamedView::Front,
                NamedView::Back,
                NamedView::Left,
                NamedView::Right,
                NamedView::Top,
                NamedView::Bottom,
                NamedView::TopFrontRight,
            ]
        );
        // Each drawn edge's middle (its ends' mean, by hand) is one edge view's point, and
        // that view's camera is the one the cube gave for the edge before it asked the core
        // (#471's: its middle times 1/√2 toward the eye, up z made perpendicular to it), to
        // the bit; each corner of the cube one corner view's.
        let bounds = [[10.0, -20.0, 5.0], [70.0, 15.0, 45.0]];
        let points: Vec<NamedView> = cube_points().collect();
        for edge in cube_edges() {
            let [start, end] = edge.ends;
            let middle: [f64; 3] = std::array::from_fn(|k| (start[k] + end[k]) / 2.0);
            let mut found = points.iter().filter(|view| view.axes() == middle);
            let (Some(view), None) = (found.next(), found.next()) else {
                panic!("{middle:?}: no one view there");
            };
            // #471's, written out: `t` the middle times 1/√2, its up z − (z·t) t made a unit.
            let toward = middle.map(|c| c * std::f64::consts::FRAC_1_SQRT_2);
            let z = [0.0, 0.0, 1.0];
            let across = sub(z, toward.map(|c| c * dot(z, toward)));
            let length = dot(across, across).sqrt();
            let was = navigate::look_at(
                toward,
                across.map(|c| c / length),
                bounds,
                1000,
                800,
                navigate::Inset::default(),
            );
            assert_eq!(navigate::named(*view, bounds, 1000, 800), was, "{view:?}");
            assert!(was.is_some());
            for corner in edge.ends {
                assert_eq!(
                    points.iter().filter(|view| view.axes() == corner).count(),
                    1
                );
            }
        }
        // The edges' points come first, so a corner's square, taken later, wins where they
        // meet: twelve two-face views, then eight three-face ones.
        let between: Vec<usize> = points.iter().map(|view| view.faces()).collect();
        assert_eq!(between, [[2; 12].as_slice(), &[3; 8]].concat());
        // Its words, as the cube labels its parts: an edge's two faces, as a corner's three.
        let words: Vec<String> = points[..12]
            .iter()
            .map(|v| crate::words::view_words(*v))
            .collect();
        assert_eq!(
            words,
            [
                "Top front",
                "Top back",
                "Top left",
                "Top right",
                "Bottom front",
                "Bottom back",
                "Bottom left",
                "Bottom right",
                "Front left",
                "Front right",
                "Back left",
                "Back right",
            ]
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
            edge: None,
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
    fn a_shown_sketchs_far_lines_raise_the_least_view_and_zoom_stops_there() {
        // No body, 1,600 rows, onto (20, 10, 5): a shown sketch's lines at (320, 10, 5),
        // 300 mm from the target, make the least 1,600 · 300 / 2^19 · 1.01 = 0.924683 mm. A
        // view at 0.01 is raised to it when the lines' boxes come, and a zoom in from there
        // stays; their boxes gone, a zoom in goes on.
        let mut view = View {
            rows: 1600,
            ..View::default()
        };
        view.set_camera(front(0.01));
        view.take_drawn_boxes(
            lines_only(&[[[320.0, 10.0, 5.0], [320.0, 10.0, 5.0]]]),
            [800, 1600],
        );
        let least = 1600.0 * 300.0 / 524_288.0 * 1.01;
        let Projection::Orthographic { view_height } = view.camera.projection else {
            panic!("orthographic")
        };
        assert!((view_height - least).abs() < 1e-12, "{view_height}");
        view.zoom(200.0, [0.0, 0.0], 800.0);
        assert_eq!(
            view.camera.projection,
            Projection::Orthographic { view_height }
        );
        view.take_drawn_boxes(DrawnBoxes::default(), [800, 1600]);
        view.zoom(200.0, [0.0, 0.0], 800.0);
        assert_eq!(
            view.camera.projection,
            Projection::Orthographic {
                view_height: view_height / 2.0
            }
        );
    }

    /// A shown sketch's boxes as a frame gives them: its lines' and its whole, `boxes`, the
    /// same here (a sketch of lines alone); no plane.
    fn lines_only(boxes: &[[[f64; 3]; 2]]) -> DrawnBoxes {
        DrawnBoxes {
            sketch_lines: boxes.to_vec(),
            sketch_fits: boxes.to_vec(),
            planes: Vec::new(),
        }
    }

    /// The 50 mm origin plane's box about the origin.
    const PLANE: [[f64; 3]; 2] = [[-25.0, -25.0, 0.0], [25.0, 25.0, 0.0]];

    /// A shown sketch's lines from (300, 0, 0) to (340, 0, 20), on XZ.
    const FAR: [[f64; 3]; 2] = [[300.0, 0.0, 0.0], [340.0, 0.0, 20.0]];

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9)
    }

    fn view_height(camera: &Camera) -> f64 {
        let Projection::Orthographic { view_height } = camera.projection else {
            panic!("orthographic")
        };
        view_height
    }

    #[test]
    fn fit_home_and_look_at_take_a_shown_sketchs_whole_and_never_a_planes_box() {
        // No body; [`FAR`] shown, and the origin plane. By hand (`views::fit_along`): Fit from
        // the front, in an 800 by 800 pane, centres FAR, (320, 0, 10), its view 1.1 times the
        // larger of its 20 mm up and its 40 mm across, 44 mm; the plane taken too would centre
        // (157.5, 0, 10). Home centres the same box from its own direction, and Look At
        // square to XZ from −y, z up, the same as Fit from the front.
        let mut view = View {
            rows: 800,
            ..View::default()
        };
        view.set_camera(front(100.0));
        let mut drawn = lines_only(&[FAR]);
        drawn.planes = vec![PLANE];
        view.take_drawn_boxes(drawn, [800, 800]);
        view.give(ViewAsk::Fit, [800, 800], Motion::JUMP);
        assert!(
            near(view.camera.target, [320.0, 0.0, 10.0]),
            "{:?}",
            view.camera
        );
        assert!((view_height(&view.camera) - 44.0).abs() < 1e-9);
        // Looking as it looked: along y, z up.
        assert!(near(view.camera.up, [0.0, 0.0, 1.0]), "{:?}", view.camera);
        view.set_camera(front(100.0));
        view.give(ViewAsk::Named(HOME), [800, 800], Motion::JUMP);
        assert!(
            near(view.camera.target, [320.0, 0.0, 10.0]),
            "{:?}",
            view.camera
        );
        view.set_camera(front(100.0));
        let look = ViewAsk::LookAt {
            toward: [0.0, -1.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        view.give(look, [800, 800], Motion::JUMP);
        assert!(
            near(view.camera.target, [320.0, 0.0, 10.0]),
            "{:?}",
            view.camera
        );
        assert!((view_height(&view.camera) - 44.0).abs() < 1e-9);
        // A fit takes a sketch by its whole, not its lines as drawn (a circle's chords fall
        // inside it, and move with the scale): lines inside FAR, FAR whole, centre FAR's.
        view.take_drawn_boxes(
            DrawnBoxes {
                sketch_lines: vec![[[301.0, 0.0, 1.0], [339.0, 0.0, 19.0]]],
                sketch_fits: vec![FAR],
                planes: Vec::new(),
            },
            [800, 800],
        );
        view.set_camera(front(100.0));
        view.give(ViewAsk::Fit, [800, 800], Motion::JUMP);
        assert!((view_height(&view.camera) - 44.0).abs() < 1e-9);
        // With no sketch shown, nothing to fit: the view stays.
        view.take_drawn_boxes(
            DrawnBoxes {
                planes: vec![PLANE],
                ..DrawnBoxes::default()
            },
            [800, 800],
        );
        view.set_camera(front(100.0));
        view.give(ViewAsk::Fit, [800, 800], Motion::JUMP);
        assert_eq!(view.camera, front(100.0));
    }

    #[test]
    fn fit_on_a_shown_sketch_drawn_as_a_point_or_a_line_end_on_stays_drawable() {
        // Break-it: a shown sketch whose lines are one point (a side of no length, as an
        // invalid sketch's members are drawn as stored) has no extent to fit: Fit leaves the
        // view as it was, never a view of no height, and Home and Look At give the cube's, as
        // with nothing to fit. One line seen end on, (20, 0, 5) to (20, 40, 5) from the
        // front: no extent across or up, so its view is the least the renderer draws its
        // 20 mm half length at, 800 · 20 / 2^19 · 1.01, onto its middle.
        let mut view = View {
            rows: 800,
            ..View::default()
        };
        view.set_camera(front(100.0));
        view.take_drawn_boxes(
            lines_only(&[[[5.0, 5.0, 5.0], [5.0, 5.0, 5.0]]]),
            [800, 800],
        );
        view.give(ViewAsk::Fit, [800, 800], Motion::JUMP);
        assert_eq!(view.camera, front(100.0));
        view.give(ViewAsk::Named(HOME), [800, 800], Motion::JUMP);
        let cube = navigate::named_in(HOME, NO_BOX, 800, 800, navigate::Inset::default());
        assert_eq!(Some(view.camera.clone()), cube);
        view.set_camera(front(100.0));
        let look = ViewAsk::LookAt {
            toward: [0.0, -1.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        view.give(look, [800, 800], Motion::JUMP);
        assert_ne!(view.camera, front(100.0));
        assert!(view.camera.check(800, 800).is_ok());
        view.take_drawn_boxes(
            lines_only(&[[[20.0, 0.0, 5.0], [20.0, 40.0, 5.0]]]),
            [800, 800],
        );
        view.give(ViewAsk::Fit, [800, 800], Motion::JUMP);
        assert_eq!(view.camera.target, [20.0, 20.0, 5.0]);
        let least = 800.0 * 20.0 / 524_288.0 * 1.01;
        let height = view_height(&view.camera);
        assert!((height - least).abs() < 1e-12, "{height} for {least}");
        assert!(view.camera.check(800, 800).is_ok());
    }

    #[test]
    fn the_opening_view_fits_home_again_as_shown_sketches_come_till_it_is_moved() {
        // Opened at Home's view of the cube (no body to fit), [`FAR`] then drawn: Home again
        // with it, onto (320, 0, 10). It drawn whole a frame on, (300, 0, 0) to (360, 0, 20):
        // Home again, onto (330, 0, 10). Moved, then another change: kept where it was moved.
        let mut view = View {
            rows: 800,
            ..View::default()
        };
        view.opened([800, 800]);
        let opened = view.camera.clone();
        view.take_drawn_boxes(lines_only(&[FAR]), [800, 800]);
        assert!(
            near(view.camera.target, [320.0, 0.0, 10.0]),
            "{:?}",
            view.camera
        );
        assert!(
            near(view_direction(&view.camera), view_direction(&opened)),
            "Home's direction"
        );
        let wider = [[300.0, 0.0, 0.0], [360.0, 0.0, 20.0]];
        view.take_drawn_boxes(lines_only(&[wider]), [800, 800]);
        assert!(
            near(view.camera.target, [330.0, 0.0, 10.0]),
            "{:?}",
            view.camera
        );
        // The lines alone changing (a new scale's chords), the whole the same: no fit again.
        let at = view.camera.clone();
        view.take_drawn_boxes(
            DrawnBoxes {
                sketch_lines: vec![[[301.0, 0.0, 1.0], [359.0, 0.0, 19.0]]],
                sketch_fits: vec![wider],
                planes: Vec::new(),
            },
            [800, 800],
        );
        assert_eq!(view.camera, at);
        view.set_camera(front(100.0));
        view.take_drawn_boxes(lines_only(&[FAR]), [800, 800]);
        assert_eq!(view.camera, front(100.0));
    }

    /// The unit vector from `camera`'s target to its eye.
    fn view_direction(camera: &Camera) -> [f64; 3] {
        let d = [0, 1, 2].map(|k| camera.eye[k] - camera.target[k]);
        let n = d.iter().map(|x| x * x).sum::<f64>().sqrt();
        d.map(|x| x / n)
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
    fn a_new_bodies_table_has_the_selections_tinted_faces_worked_out_again() {
        // Body m3tq8v0z, two faces, selected by its row: both its faces tinted as its
        // maker's, as body 1, none highlighted. A read's answer then numbers it 2 (a body
        // before it, new): the table is made again, and the same selection tints body 2's
        // faces, not body 1's.
        use silico_core::naming::{Name, PrimitiveKind, TypeWord};
        use silico_protocol::{ElementKind, ElementRef};
        let id = FeatureId::new("m3tq8v0z").unwrap();
        let faces: Vec<MeshElement> = (1..=2)
            .map(|index| MeshElement {
                name: Arc::new(Name::Element {
                    feature: id.clone(),
                    made: TypeWord::Primitive(PrimitiveKind::Box),
                    element: ElementRef {
                        kind: ElementKind::Face,
                        index,
                    },
                }),
                stable: true,
                count: None,
            })
            .collect();
        let mut first = body(0.03125, [0.03125, 0.0625], CUT);
        first.faces = faces.clone();
        let mut view = view_with(&[(1, first)], 3);
        let mut selection = crate::selection::Selection::default();
        selection.select_feature(id);
        let frame = ViewFrame {
            session: SessionId::parse("s1").unwrap(),
            rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0)),
            camera: front(80.0),
            size_px: [100, 100],
            pixels_per_point: 1.0,
            pointer_px: None,
            left: Vec::new(),
            secondary: Vec::new(),
            drawn: Vec::new(),
            now: now(),
        };
        let ctx = egui::Context::default();
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        let lit = |body: u32| BTreeSet::from([(body, Element::Face(1)), (body, Element::Face(2))]);
        assert_eq!(view.selected_feature_faces, lit(1));
        assert_eq!(view.maker_faces(), Vec::from_iter(lit(1)));
        assert!(view.highlights.is_empty());
        let mesh = Arc::clone(&view.bodies[&1].mesh);
        view.take_bodies(vec![MeshedBody {
            faces,
            ..meshed(2, 1, 0.03125, Some(mesh))
        }]);
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(view.selected_feature_faces, lit(2));
    }

    #[test]
    fn one_feature_tints_the_same_faces_hovered_in_the_timeline_or_selected_and_its_body_all() {
        // Body m3tq8v0z, the feature's own: its Face1 named as aaaaaaaa's, its Face2 as its
        // own. Selected by its timeline item (`select_feature`, the prototype's
        // `selectFeature`: the maker is the feature), it tints its Face2 alone, as hovered in
        // the timeline it does; selected by its browser row (`select_body`, `bodyRow`'s
        // `maker: '*'`), the body's both.
        use silico_core::naming::{Name, PrimitiveKind, TypeWord};
        use silico_protocol::{ElementKind, ElementRef};
        let named = |id: &str, index: u32| MeshElement {
            name: Arc::new(Name::Element {
                feature: FeatureId::new(id).unwrap(),
                made: TypeWord::Primitive(PrimitiveKind::Box),
                element: ElementRef {
                    kind: ElementKind::Face,
                    index,
                },
            }),
            stable: true,
            count: None,
        };
        let mut held = body(0.03125, [0.03125, 0.0625], CUT);
        held.faces = vec![named("aaaaaaaa", 1), named("m3tq8v0z", 2)];
        let mut view = view_with(&[(1, held)], 3);
        let id = FeatureId::new("m3tq8v0z").unwrap();
        let frame = ViewFrame {
            session: SessionId::parse("s1").unwrap(),
            rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0)),
            camera: front(80.0),
            size_px: [100, 100],
            pixels_per_point: 1.0,
            pointer_px: None,
            left: Vec::new(),
            secondary: Vec::new(),
            drawn: Vec::new(),
            now: now(),
        };
        let ctx = egui::Context::default();
        let mut selection = crate::selection::Selection::default();
        selection.select_feature(id.clone());
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        let selected = view.maker_faces();
        assert_eq!(selected, [(1, Element::Face(2))]);
        view.linked = Some(id.clone());
        assert_eq!(view.maker_faces(), selected, "selected against hovered");
        view.linked = None;
        selection.select_bodies(vec![id.clone()]);
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(
            view.maker_faces(),
            [(1, Element::Face(1)), (1, Element::Face(2))]
        );
        // Its timeline item clicked again: itself, its own face.
        selection.select_feature(id.clone());
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(view.maker_faces(), [(1, Element::Face(2))]);
        // A dialog's body box (Union's) takes it as its body: whole.
        let whole = [(1, Element::Face(1)), (1, Element::Face(2))];
        selection.select_bodies(vec![id.clone()]);
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(view.maker_faces(), whole, "a dialog's body box");
        // Selected by its row, then a dialog taking edges alone, then its Cancel: whole.
        selection.select_bodies(vec![id.clone()]);
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(view.maker_faces(), whole);
        let saved = selection.saved();
        selection.keep_only(crate::selection::Filter {
            faces: false,
            edges: true,
            vertices: false,
        });
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert!(view.maker_faces().is_empty());
        selection.restore(saved);
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(view.maker_faces(), whole, "after the dialog's Cancel");
        // Selected by its timeline item, then a dialog's body box, then its Cancel: its own
        // face again.
        selection.select_feature(id.clone());
        let saved = selection.saved();
        selection.select_bodies(vec![id]);
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(view.maker_faces(), whole);
        selection.restore(saved);
        selection.overlay(&mut view, &frame, &ctx, &crate::selection::no_face);
        assert_eq!(
            view.maker_faces(),
            [(1, Element::Face(2))],
            "after the body box's Cancel"
        );
    }

    #[test]
    fn a_new_bodies_table_lets_a_hover_and_a_candidate_by_its_old_numbers_go() {
        // Body 1 lit for the table it came in, Face1 drawn hovered and Face2 a candidate.
        // A read's answer then numbers it 2: before the frame goes out, the selection's
        // light lets both go (they're by the old numbers, body 1 being another body now).
        // A menu holding the hover keeps its own row; the candidate goes either way.
        let mut first = body(0.03125, [0.03125, 0.0625], CUT);
        first.faces = Vec::new();
        for held in [false, true] {
            let mut view = view_with(&[(1, first.clone())], 3);
            let mut selection = crate::selection::Selection::default();
            selection.light(&mut view);
            if held {
                selection.hold_hover(None);
            }
            view.hovered = BTreeSet::from([(1, Element::Face(1))]);
            view.candidate = Some((1, Element::Face(2)));
            // The same table: nothing let go.
            selection.light(&mut view);
            assert_eq!(view.hovered, BTreeSet::from([(1, Element::Face(1))]));
            assert_eq!(view.candidate, Some((1, Element::Face(2))));
            let mesh = Arc::clone(&view.bodies[&1].mesh);
            view.take_bodies(vec![meshed(2, 1, 0.03125, Some(mesh))]);
            selection.light(&mut view);
            assert_eq!(view.candidate, None, "held {held}");
            if held {
                assert_eq!(view.hovered, BTreeSet::from([(1, Element::Face(1))]));
            } else {
                assert!(view.hovered.is_empty());
            }
        }
    }

    #[test]
    fn a_selection_change_on_the_same_table_keeps_the_hover_and_the_candidate() {
        // The table unchanged and the selection changed between two lights (a feature
        // selected): its highlights and tints are worked out again, but the hover drawn and a
        // lit candidate are of this table's numbers still, and stay.
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let mut selection = crate::selection::Selection::default();
        selection.light(&mut view);
        view.hovered = BTreeSet::from([(1, Element::Face(1))]);
        view.candidate = Some((1, Element::Face(2)));
        let before = selection.version();
        selection.select_feature(FeatureId::new("m3tq8v0z").unwrap());
        assert_ne!(selection.version(), before);
        selection.light(&mut view);
        assert_eq!(view.hovered, BTreeSet::from([(1, Element::Face(1))]));
        assert_eq!(view.candidate, Some((1, Element::Face(2))));
    }

    #[test]
    fn the_maker_tinted_is_the_feature_hovered_in_the_timeline_else_those_selected() {
        // viewport.js's `syncHighlight`: `maker = S.hoverFeature || S.selFeature`. Body 1's
        // faces 1 and 2 made by aaaaaaaa, its face 3 by bbbbbbbb; aaaaaaaa selected. Nothing
        // hovered in the timeline: aaaaaaaa's two. bbbbbbbb hovered: its face alone, the
        // selected one's left. cccccccc hovered, which made none: none, not the selected's.
        use silico_core::naming::{Name, PrimitiveKind, TypeWord};
        use silico_protocol::{ElementKind, ElementRef};
        let named = |id: &str, index: u32| MeshElement {
            name: Arc::new(Name::Element {
                feature: FeatureId::new(id).unwrap(),
                made: TypeWord::Primitive(PrimitiveKind::Box),
                element: ElementRef {
                    kind: ElementKind::Face,
                    index,
                },
            }),
            stable: true,
            count: None,
        };
        let mut held = body(0.03125, [0.03125, 0.0625], CUT);
        held.faces = vec![
            named("aaaaaaaa", 1),
            named("aaaaaaaa", 2),
            named("bbbbbbbb", 3),
        ];
        let mut view = view_with(&[(1, held)], 3);
        view.selected_feature_faces =
            BTreeSet::from([(1, Element::Face(1)), (1, Element::Face(2))]);
        assert_eq!(
            view.maker_faces(),
            [(1, Element::Face(1)), (1, Element::Face(2))]
        );
        view.linked = Some(FeatureId::new("bbbbbbbb").unwrap());
        assert_eq!(view.maker_faces(), [(1, Element::Face(3))]);
        view.linked = Some(FeatureId::new("cccccccc").unwrap());
        assert!(view.maker_faces().is_empty());
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
        // A vertex by its point's number: two points, Vertex1 and Vertex2.
        held.mesh = Arc::new(Mesh {
            vertex_points: vec![[0.0; 3], [1.0; 3]],
            ..(*held.mesh).clone()
        });
        assert!(held.has(Element::Vertex(1)) && held.has(Element::Vertex(2)));
        assert!(!held.has(Element::Vertex(0)) && !held.has(Element::Vertex(3)));
        assert!(!held.has(Element::Vertex(u32::MAX)));
        assert_eq!(held.vertex_point(2), Some([1.0; 3]));
        assert_eq!(held.vertex_point(0), None);
        assert_eq!(held.vertex_point(3), None);
    }

    #[test]
    fn a_vertex_is_named_by_its_place_among_the_bodys_vertices() {
        use silico_core::naming::{Name, PrimitiveKind, TypeWord};
        use silico_protocol::{ElementKind, ElementRef};
        let element = |kind, index| MeshElement {
            name: Arc::new(Name::Element {
                feature: FeatureId::new("m3tq8v0z").unwrap(),
                made: TypeWord::Primitive(PrimitiveKind::Box),
                element: ElementRef { kind, index },
            }),
            stable: true,
            count: None,
        };
        let mut body = body(0.03125, [0.03125, 0.0625], CUT);
        body.faces = vec![element(ElementKind::Face, 1)];
        body.edges = vec![element(ElementKind::Edge, 1)];
        body.vertices = vec![
            element(ElementKind::Vertex, 1),
            element(ElementKind::Vertex, 2),
        ];
        let named = |e: Element| body.name_of(e).map(|m| m.name.to_string());
        assert_eq!(
            named(Element::Vertex(2)).as_deref(),
            Some("m3tq8v0z.box.Vertex2")
        );
        assert_eq!(
            named(Element::Vertex(1)).as_deref(),
            Some("m3tq8v0z.box.Vertex1")
        );
        // Each kind its own list: Vertex1 isn't Face1 or Edge1.
        assert_eq!(
            named(Element::Face(1)).as_deref(),
            Some("m3tq8v0z.box.Face1")
        );
        assert_eq!(
            named(Element::Edge(1)).as_deref(),
            Some("m3tq8v0z.box.Edge1")
        );
        for missing in [0, 3, u32::MAX] {
            assert_eq!(named(Element::Vertex(missing)), None, "{missing}");
        }
    }

    /// The corner's view (M5.7 part 3, §4): orthographic about (10, 10, 10) from along
    /// (1, 1, 1), z up, 80 mm high.
    fn corner_camera(projection: Projection) -> Camera {
        Camera {
            projection,
            eye: [110.0, 110.0, 110.0],
            target: [10.0, 10.0, 10.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn a_vertexs_dot_sits_where_the_renderer_draws_its_point_and_none_nearer_the_eye_than_that() {
        // A viewport 400 points square from (10, 20), at two pixels a point: 800 pixels. The
        // corner (20, 0, 20) lands at pixel (400 − 200/√2, 400 − 200/√6), by hand, so at
        // (10 + 129.289, 20 + 159.175) points.
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::splat(400.0));
        let camera = corner_camera(Projection::Orthographic { view_height: 80.0 });
        let corner = [20.0, 0.0, 20.0];
        let target = [10.0, 10.0, 10.0];
        let got = dots_at(
            rect,
            &camera,
            [800, 800],
            2.0,
            &[(corner, false), (target, true)],
        );
        let want = [
            (
                10.0 + (400.0 - 200.0 / 2f64.sqrt()) / 2.0,
                20.0 + (400.0 - 200.0 / 6f64.sqrt()) / 2.0,
                false,
            ),
            (210.0, 220.0, true),
        ];
        assert_eq!(got.len(), 2);
        for ((at, selected), (x, y, want_selected)) in got.iter().zip(want) {
            assert!(
                (f64::from(at.x) - x).abs() < 1e-3 && (f64::from(at.y) - y).abs() < 1e-3,
                "{at:?} against ({x}, {y})"
            );
            assert_eq!(*selected, want_selected);
        }
        // In perspective, a point nearer the eye than the renderer draws (0.75 nm along the
        // view, under its 1 nm) has no dot, as it has no disc; 2 nm, it has.
        let u = [1.0, -1.0, 1.0].map(|c: f64| c / 3f64.sqrt());
        let looking = |off: f64| Camera {
            projection: Projection::Perspective {
                field_of_view: 60.0,
            },
            eye: std::array::from_fn(|k| corner[k] + u[k] * off),
            target: std::array::from_fn(|k| corner[k] - u[k]),
            up: [0.0, 0.0, 1.0],
        };
        let near = looking(0.75 * silico_protocol::MIN_EYE_DISTANCE);
        assert!(dots_at(rect, &near, [800, 800], 2.0, &[(corner, true)]).is_empty());
        let far = looking(2.0 * silico_protocol::MIN_EYE_DISTANCE);
        let got = dots_at(rect, &far, [800, 800], 2.0, &[(corner, true)]);
        assert_eq!(got.len(), 1);
        assert!(
            (got[0].0 - Pos2::new(210.0, 220.0)).length() < 1e-3,
            "{got:?}"
        );
    }

    #[test]
    fn a_dot_whose_place_in_points_overflows_on_either_side_has_none() {
        // The corner camera's right is (−1, 1, 0)/√2 and its up (−1, −1, 2)/√6, at 10 pixels
        // a mm: a point 10³⁸ mm along either from the target lands 10³⁹ pixels out on that
        // side alone, past f32's 3.4 × 10³⁸ in points (at two pixels a point, 5 × 10³⁸);
        // its other side is the centre, finite. Neither has a dot.
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::splat(400.0));
        let camera = corner_camera(Projection::Orthographic { view_height: 80.0 });
        let far = 1e38;
        let right = [10.0 - far / 2f64.sqrt(), 10.0 + far / 2f64.sqrt(), 10.0];
        let up = [
            10.0 - far / 6f64.sqrt(),
            10.0 - far / 6f64.sqrt(),
            10.0 + 2.0 * far / 6f64.sqrt(),
        ];
        for point in [right, up] {
            let pixel = silico_render::render::project(&camera, [800, 800], point).unwrap();
            assert!(pixel.iter().all(|c| c.is_finite()), "{pixel:?}");
            assert!(pixel.iter().any(|c| c.abs() > 1e39 * 0.99), "{pixel:?}");
            assert!(dots_at(rect, &camera, [800, 800], 2.0, &[(point, true)]).is_empty());
        }
    }

    #[test]
    fn a_hovered_dot_is_white_and_a_selected_one_blue_each_4_5_points_ringed_in_black() {
        // The prototype's (DESIGN.md's *Viewport*, interact.js `overlay`'s `dot`): radius
        // 4.5 points, a 1-point #000 ring, hover #ffffff, selected #4676ff; at the corner's
        // place in points (by hand, above) and the target's, the viewport's centre.
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::splat(400.0));
        let camera = corner_camera(Projection::Orthographic { view_height: 80.0 });
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            paint_vertex_dots(
                ui.painter(),
                rect,
                &camera,
                [800, 800],
                2.0,
                &[([20.0, 0.0, 20.0], false), ([10.0, 10.0, 10.0], true)],
            );
        });
        output.textures_delta.clear();
        let circles: Vec<_> = output
            .shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Circle(circle) => Some(circle),
                _ => None,
            })
            .collect();
        let corner = Pos2::new(
            (10.0 + (400.0 - 200.0 / 2f64.sqrt()) / 2.0) as f32,
            (20.0 + (400.0 - 200.0 / 6f64.sqrt()) / 2.0) as f32,
        );
        let want = [
            (corner, egui::Color32::from_rgb(0xff, 0xff, 0xff)),
            (
                Pos2::new(210.0, 220.0),
                egui::Color32::from_rgb(0x46, 0x76, 0xff),
            ),
        ];
        assert_eq!(circles.len(), 2, "{circles:?}");
        for (circle, (at, fill)) in circles.iter().zip(want) {
            assert!((circle.center - at).length() < 1e-3, "{circle:?}");
            assert_eq!((circle.radius, circle.fill), (4.5, fill));
            assert_eq!(circle.stroke, Stroke::new(1.0, egui::Color32::BLACK));
        }
    }

    #[test]
    fn a_hovered_features_faces_are_tinted_but_those_selected_or_hovered_and_those_not_drawn() {
        use Element::{Edge, Face};
        // The feature's faces as found, out of order and one twice: body 1's 1 to 4 and body
        // 2's 1 and 2. Body 1's Face2 selected and its Face3 hovered keep their looks (the
        // prototype's `applyHighlight`: the selection's, else the hover's, else the maker's
        // tint); body 2's Face2 a candidate on the hover's layer, the same. An edge selected or
        // hovered takes no face of the tint. Body 3 isn't drawn.
        let linked = vec![
            (2, Face(2)),
            (1, Face(4)),
            (1, Face(1)),
            (1, Face(2)),
            (3, Face(1)),
            (1, Face(3)),
            (2, Face(1)),
            (1, Face(1)),
        ];
        let highlights = [(1, Edge(1)), (1, Face(2))];
        let hovered = [(1, Face(3)), (2, Face(2)), (1, Edge(4))];
        let drawn = |(body, _): &(u32, Element)| *body != 3;
        assert_eq!(
            tinted_faces(linked.clone(), &highlights, &hovered, drawn),
            [(1, Face(1)), (1, Face(4)), (2, Face(1))]
        );
        // Nothing selected or hovered: every face drawn, ascending, each once.
        assert_eq!(
            tinted_faces(linked, &[], &[], drawn),
            [
                (1, Face(1)),
                (1, Face(2)),
                (1, Face(3)),
                (1, Face(4)),
                (2, Face(1)),
                (2, Face(2))
            ]
        );
        // No feature hovered: none.
        assert!(tinted_faces(Vec::new(), &highlights, &hovered, drawn).is_empty());
    }

    #[test]
    fn the_feature_tint_is_the_prototypes_blue_800_at_0_4() {
        // viewport.js's `MAKER`, `#d4dbff`, at `uTintAmt.value = 0.4` (DESIGN.md's "a
        // feature's faces on hover: Blue.800 tint 40%").
        assert_eq!(
            feature_tint(),
            Tint {
                colour: [0xd4, 0xdb, 0xff],
                amount: 0.4,
            }
        );
    }

    #[test]
    fn the_dots_are_the_hovered_vertex_then_the_selected_ones_of_bodies_held() {
        let mut held = body(0.03125, [0.03125, 0.0625], CUT);
        held.mesh = Arc::new(Mesh {
            vertex_points: vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]],
            ..(*held.mesh).clone()
        });
        let bodies = BTreeMap::from([(1, held)]);
        let dots = vertex_dots(
            &bodies,
            &[
                (1, Element::Vertex(3)),
                (1, Element::Face(1)),
                (2, Element::Vertex(1)),
                (1, Element::Vertex(4)),
                (1, Element::Vertex(1)),
            ],
            &[(1, Element::Edge(1)), (1, Element::Vertex(2))],
        );
        // The hover first, so a selected dot is drawn over it; faces, edges, a body not held
        // and a vertex its mesh hasn't, none.
        assert_eq!(
            dots,
            [
                ([2.0, 0.0, 0.0], false),
                ([3.0, 0.0, 0.0], true),
                ([1.0, 0.0, 0.0], true),
            ]
        );
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
    fn a_revisions_meshes_are_refused_while_its_last_read_was_refused_and_none_is_in_flight() {
        let at = now();
        let four = Revision::from_number(4).unwrap();
        let five = Revision::from_number(5).unwrap();
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        // Refused for what it asked (its width): of 4, kept, not to be asked again.
        let request = asked(&view, 4);
        view.took(
            request,
            false,
            refusal(ErrorCode::InvalidInput, Some("width")),
            at,
        );
        assert!(view.retry.is_none());
        assert!(view.refused_of(four));
        assert!(!view.refused_of(five));
        assert!(!view.refused_of(Revision::from_number(3).unwrap()));
        // Refused for the moment: of 5, asked again later; 4's refusal stands.
        let request = asked(&view, 5);
        view.took(request, false, refusal(ErrorCode::WorkerCrashed, None), at);
        assert!(view.refused_of(five));
        assert!(view.refused_of(four));
        // A read in flight (here of 4, its answer unread): it may yet answer, so neither is.
        let sessions = Arc::new(Sessions::new(Arc::new(
            silico_core::fake::FakeKernel::new(1).unwrap(),
        )));
        let request = asked(&view, 4);
        view.start(&sessions, &SessionId::parse("s1").unwrap(), request);
        assert!(view.reading());
        assert!(!view.refused_of(four));
        assert!(!view.refused_of(five));
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

    /// Each face's label reads from outside the cube (its right × up is its normal) and
    /// upright from the face's own named view as the core gives it: its normal the view's
    /// way, its up the view's up, its right the view's right (the up × that way).
    #[test]
    fn each_cube_faces_label_reads_upright_from_its_own_view() {
        use silico_protocol::vec3::cross;
        let mut views: Vec<NamedView> = FACES.iter().map(|face| face.view).collect();
        for face in FACES {
            let view = face.view;
            assert_eq!(cross(face.right, face.up), face.normal, "{view:?}");
            assert_eq!(view.direction(), face.normal, "{view:?}");
            assert_eq!(view.up(), face.up, "{view:?}");
            assert_eq!(cross(view.up(), view.direction()), face.right, "{view:?}");
        }
        // The six faces, each once.
        views.sort_by_key(|view| view.name());
        views.dedup();
        assert_eq!(views.len(), 6);
    }

    /// The cube's edges: its twelve, each once, along one axis from −1 to 1 at two corners,
    /// between the two faces whose normals are its other two coordinates.
    #[test]
    fn the_cubes_edges_are_its_twelve_each_once_between_the_faces_it_joins() {
        let mut seen = BTreeSet::new();
        let mut along = [0; 3];
        for CubeEdge {
            ends: [start, end],
            faces: [one, other],
        } in cube_edges()
        {
            let moving: Vec<usize> = (0..3).filter(|&k| start[k] != end[k]).collect();
            assert_eq!(moving.len(), 1, "{start:?} {end:?}");
            let k = moving[0];
            along[k] += 1;
            assert_eq!((start[k], end[k]), (-1.0, 1.0));
            assert!(start.iter().chain(&end).all(|c| c.abs() == 1.0));
            assert_ne!(one, other);
            for normal in [one, other] {
                let nonzero: Vec<usize> = (0..3).filter(|&a| normal[a] != 0.0).collect();
                assert_eq!(nonzero.len(), 1, "{normal:?}");
                let a = nonzero[0];
                assert_ne!(a, k);
                assert_eq!(normal[a], start[a], "{normal:?} at {start:?}");
            }
            assert!(seen.insert((start.map(f64::to_bits), end.map(f64::to_bits))));
            // The midpoint of its ends, their mean: on the axis it runs along 0, and on the
            // others its faces' ±1, their normals' sum.
            let midpoint: [f64; 3] = std::array::from_fn(|a| (start[a] + end[a]) / 2.0);
            let sum: [f64; 3] = std::array::from_fn(|a| one[a] + other[a]);
            assert_eq!(sum, midpoint, "{start:?} {end:?}");
        }
        assert_eq!(seen.len(), 12);
        assert_eq!(along, [4, 4, 4]);
    }

    /// The canvas's gradient: a mesh of two triangles over the rectangle, its top corners
    /// #141517 and its bottom ones #2a2c30 (styles.css's `--canvas-top` and
    /// `--canvas-bottom`).
    #[test]
    fn the_canvas_background_runs_from_its_top_colour_to_its_bottom_one() {
        let rect = Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 220.0));
        let egui::Shape::Mesh(mesh) = canvas_background(rect) else {
            panic!("not a mesh");
        };
        let top = egui::Color32::from_rgb(0x14, 0x15, 0x17);
        let bottom = egui::Color32::from_rgb(0x2a, 0x2c, 0x30);
        let corners: Vec<(Pos2, egui::Color32)> =
            mesh.vertices.iter().map(|v| (v.pos, v.color)).collect();
        assert_eq!(
            corners,
            [
                (egui::pos2(10.0, 20.0), top),
                (egui::pos2(110.0, 20.0), top),
                (egui::pos2(110.0, 220.0), bottom),
                (egui::pos2(10.0, 220.0), bottom),
            ]
        );
        assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3]);
        assert_eq!(mesh.texture_id, egui::TextureId::default());
    }

    /// `got` is the interval `want`, each end to 1e-12.
    #[track_caller]
    fn same_interval(got: Option<[f64; 2]>, want: [f64; 2]) {
        let got = got.unwrap_or_else(|| panic!("nothing hidden, against {want:?}"));
        assert!(
            (got[0] - want[0]).abs() < 1e-12 && (got[1] - want[1]).abs() < 1e-12,
            "{got:?} against {want:?}"
        );
    }

    /// The axis stubs, from (−1.02, −1.02, −1.02), 1.4 along each axis, hidden where the
    /// ray from a point toward the eye meets the cube, by hand. From the home view's way,
    /// (1, −1, 1)/√3, the x stub's point (−1.02 + 1.4·s, −1.02, −1.02) moves to y < −1 toward
    /// the eye, never in the cube: seen whole, as is the z stub. The y stub's (−1.02, y,
    /// −1.02) moves by u = t/√3 to (−1.02 + u, y − u, −1.02 + u): in the cube for u from 0.02
    /// to 2.02 and from y − 1 to y + 1, some u for y + 1 ≥ 0.02, so hidden from y = −0.98,
    /// s = 0.04/1.4 = 1/35, to its end, and seen from 0 to 1/35. From the back and above,
    /// (0, 1, 1)/√2, the x stub moves by u = t/√2 to (x, −1.02 + u, −1.02 + u), in the cube
    /// for u from 0.02 to 2.02 if its x is too, from s = 0.02/1.4 = 1/70: seen from 0 to
    /// 1/70. Square on from the front, (0, −1, 0), each stub's point moves away from the
    /// cube, or stays 0.02 outside it: none hidden.
    #[test]
    fn the_cubes_axis_stubs_are_hidden_where_the_cube_is_in_front_of_them() {
        let third = 1.0 / 3f64.sqrt();
        let home = [third, -third, third];
        let stub = |k: usize| {
            let mut along = [0.0; 3];
            along[k] = CUBE_STUB_LENGTH;
            along
        };
        assert_eq!(CUBE_STUB_FROM, [-1.02; 3]);
        assert_eq!(CUBE_STUB_LENGTH, 1.4);
        assert_eq!(hidden_by_cube(CUBE_STUB_FROM, stub(0), home), None);
        assert_eq!(hidden_by_cube(CUBE_STUB_FROM, stub(2), home), None);
        same_interval(
            hidden_by_cube(CUBE_STUB_FROM, stub(1), home),
            [1.0 / 35.0, 1.0],
        );
        assert_eq!(seen_past_cube(CUBE_STUB_FROM, stub(0), home), [[0.0, 1.0]]);
        let seen = seen_past_cube(CUBE_STUB_FROM, stub(1), home);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0][0], 0.0);
        assert!((seen[0][1] - 1.0 / 35.0).abs() < 1e-12, "{seen:?}");
        let half = 1.0 / 2f64.sqrt();
        let back_above = [0.0, half, half];
        same_interval(
            hidden_by_cube(CUBE_STUB_FROM, stub(0), back_above),
            [1.0 / 70.0, 1.0],
        );
        for k in 0..3 {
            assert_eq!(
                hidden_by_cube(CUBE_STUB_FROM, stub(k), [0.0, -1.0, 0.0]),
                None,
                "{k}"
            );
        }
        // A segment through the cube's middle, along x at y = z = 0 from −2 to 2, seen from
        // the front: hidden where it's in the cube, x from −1 to 1, s from 1/4 to 3/4, and
        // seen either side of that.
        let through = ([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0]);
        same_interval(
            hidden_by_cube(through.0, through.1, [0.0, -1.0, 0.0]),
            [0.25, 0.75],
        );
        assert_eq!(
            seen_past_cube(through.0, through.1, [0.0, -1.0, 0.0]),
            [[0.0, 0.25], [0.75, 1.0]]
        );
        // The same segment 0.5 in front of the cube: never hidden.
        assert_eq!(
            hidden_by_cube([-2.0, -1.5, 0.0], [4.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            None
        );
        // And 0.5 behind it: hidden where the cube's in front, s from 1/4 to 3/4.
        same_interval(
            hidden_by_cube([-2.0, 1.5, 0.0], [4.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            [0.25, 0.75],
        );
    }

    /// The cube's far face and its own faces, by hand. Along y at x = z = 0, from y = 1.5
    /// to 5, seen from (0.6, −0.8, 0): a point (0, y, 0) moves to (0.6t, y − 0.8t, 0) toward
    /// the eye, reaching the cube's far face, y = 1, at t = (y − 1)/0.8, and leaving it by
    /// its side, x = 1, at t = 1/0.6: hidden while (y − 1)/0.8 ≤ 5/3, to y = 7/3, s =
    /// (7/3 − 3/2)/3.5 = 5/21, and seen past that. The cube is closed: behind it from the
    /// front, along x from −2 to 2 at y = 1.5, a segment in the plane of its top face, z = 1,
    /// or its bottom's, z = −1, is hidden where the cube's in front, s from 1/4 to 3/4; one
    /// 0.5 above it or below it, never.
    #[test]
    fn the_cube_hides_a_segment_as_far_as_its_far_face_and_its_closed_faces_reach() {
        same_interval(
            hidden_by_cube([0.0, 1.5, 0.0], [0.0, 3.5, 0.0], [0.6, -0.8, 0.0]),
            [0.0, 5.0 / 21.0],
        );
        let seen = seen_past_cube([0.0, 1.5, 0.0], [0.0, 3.5, 0.0], [0.6, -0.8, 0.0]);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!((seen[0][0] - 5.0 / 21.0).abs() < 1e-12, "{seen:?}");
        assert_eq!(seen[0][1], 1.0);
        let front = [0.0, -1.0, 0.0];
        for z in [1.0, -1.0] {
            same_interval(
                hidden_by_cube([-2.0, 1.5, z], [4.0, 0.0, 0.0], front),
                [0.25, 0.75],
            );
        }
        for z in [1.5, -1.5] {
            assert_eq!(
                hidden_by_cube([-2.0, 1.5, z], [4.0, 0.0, 0.0], front),
                None,
                "{z}"
            );
        }
    }

    /// A label left waiting by a run that never ended a pass for it is dropped, not drawn
    /// over the next run's shapes. egui takes a run's plugins before its first pass, so the
    /// labels' plugin, registered by the first pass that labels a face, ends no pass of that
    /// run: if that pass asks for another, the second's label waits, keeping a place in a
    /// paint list that's gone. The next run, which draws only a red square, where that label
    /// kept its place, still draws the square, and no label; the one after, labelling the
    /// face, draws its label.
    #[test]
    fn a_label_left_waiting_by_a_run_that_never_ended_its_pass_is_dropped_not_drawn_over_the_next()
    {
        let ctx = egui::Context::default();
        // The app's fonts: eframe's default ones aren't built in, so with none a label lays out
        // no glyphs.
        theme::apply(&ctx);
        let on = OnFace {
            centre: egui::pos2(50.0, 50.0),
            across: Vec2::new(20.0, 0.0),
            down: Vec2::new(0.0, 20.0),
        };
        let square = egui::Color32::RED;
        let mut passes = 0;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            passes += 1;
            label_cube_face(ui.painter(), on, 20.0, "TOP");
            if passes == 1 {
                ui.ctx().request_discard("a second pass");
            }
        })
        .drop_without_applying_deltas();
        assert_eq!(passes, 2);
        assert_eq!(
            ctx.with_plugin(|labels: &mut CubeLabels| labels.waiting.len()),
            Some(1)
        );
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.painter().rect_filled(
                Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::splat(10.0)),
                0.0,
                square,
            );
        });
        let shapes: Vec<egui::Shape> = output.shapes.iter().map(|c| c.shape.clone()).collect();
        output.drop_without_applying_deltas();
        assert!(
            shapes
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Rect(r) if r.fill == square)),
            "{shapes:?}"
        );
        assert!(
            !shapes
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Mesh(_))),
            "{shapes:?}"
        );
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            label_cube_face(ui.painter(), on, 20.0, "TOP");
        });
        let meshes = output
            .shapes
            .iter()
            .filter(|c| matches!(&c.shape, egui::Shape::Mesh(m) if !m.vertices.is_empty()))
            .count();
        let shapes = format!("{:?}", output.shapes);
        output.drop_without_applying_deltas();
        assert_eq!(meshes, 1, "{shapes}");
    }

    #[test]
    fn a_read_answered_within_is_taken_for_the_next_poll_and_none_out_is_none_answered() {
        // #565's mutants: with no read out there's nothing to answer, whatever the wait.
        // A read of a session that isn't open answers at once (refused): it has answered,
        // and is out no more, so a second wait finds none.
        let mut view = View::default();
        assert!(!view.read_answered_within(Duration::ZERO));
        assert!(!view.read_answered_within(Duration::from_millis(10)));
        let sessions = Arc::new(Sessions::new(Arc::new(
            silico_core::fake::FakeKernel::new(1).unwrap(),
        )));
        let request = asked(&view, 4);
        view.start(&sessions, &SessionId::parse("s1").unwrap(), request);
        assert!(view.reading());
        assert!(view.read_answered_within(Duration::from_secs(60)));
        assert!(!view.reading());
        assert!(!view.read_answered_within(Duration::ZERO));
    }
}
