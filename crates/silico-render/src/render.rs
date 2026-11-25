//! A [`RenderScene`] drawn headless (docs/design/m4.6-render.md, as docs/design/m5.0-app.md
//! section 10 amends it): its bodies' faces and edges, its highlights in Fusion's selection
//! blue, read back as an 8-bit sRGB PNG and, when asked, the ID buffer. And the app's
//! viewport, drawn the same way a frame at a time.
//!
//! The camera stays fixed in the world and the bodies move. Each face's mesh is uploaded
//! once, its positions about its own box's centre (subtracted in f64 before they're f32) and
//! its normals its own, and each view moves it into place by its instance's transform: the
//! view's rotation and the centre's offset from the eye, computed in f64 and then f32
//! ([`world_from_mesh`]). So a new view of the same bodies uploads nothing: it builds only
//! the instances and the edges' line strips, which are placed in the view each frame in
//! f64, precise at every distance. Only a view so close that f32 about a face's centre would
//! miss its pixels (a large face seen close) draws that face about the camera's point
//! instead, uploaded for that view, as M4.6 drew every view. The light is `re_renderer`'s
//! own (an ambient and two lights fixed in the world), per pixel, which the fixed camera
//! makes the camera's.
//!
//! Drawing is separate from the device: [`device`] makes the worker's on its adapter, and
//! [`Renderer::new`] draws on any (the app's, eframe's).
//!
//! The app's viewport draws through the same path, one frame at a time and never waiting
//! for the GPU (section 10's "one path for the worker and the app"): each body's mesh is
//! given once ([`Renderer::upload`], and [`Renderer::forget`]), then each frame is built
//! and drawn into `re_renderer`'s own target ([`Renderer::prepare`], in egui_wgpu's
//! `prepare`), composited into egui's pass ([`Renderer::composite`], in its `paint`), and
//! what its picking rectangle found is read a frame or two later
//! ([`Renderer::take_picking`]). The worker's [`Renderer::render`] builds its view the
//! same way, then waits for its readbacks. The egui callback that calls them is the app's:
//! `deny.toml` keeps egui-wgpu out of this crate.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use re_renderer::mesh::{CpuMesh, GpuMaterial, GpuMesh, Material};
use re_renderer::renderer::{GpuMeshInstance, MeshDrawData, PointCloudBatchFlags};
use re_renderer::view_builder::{
    BlendWithBackground, OrthographicCameraMode, Projection as ViewProjection, TargetConfiguration,
    ViewBuilder, ViewBuilderId, ViewPickingConfiguration,
};
use re_renderer::{
    Color32, LineDrawableBuilder, OutlineConfig, OutlineMaskPreference, PickingLayerId,
    PickingLayerInstanceId, PickingLayerObjectId, PickingLayerProcessor, PointCloudBuilder,
    RectInt, RenderConfig, RenderContext, Rgba, Rgba32Unmul, ScreenshotProcessor, Size,
};
use silico_protocol::vec3::{cross, dot, sub};
use silico_protocol::{
    Camera, Class, EDGE_BIT, F32_ROUNDING, Frame as CameraFrame, InView, InViewBody,
    InvalidParameter, MAX_RENDER_COORDINATE, MIN_EYE_DISTANCE, Mesh, PRECISION_OF_PIXEL, PixelId,
    Projection, RenderBody, RenderScene, ViewRegion, box_corners, box_of, check_bodies_in_view,
    check_render_mesh, corner_sec2, least_view_height, pyramid_slope, reach_of_box,
    reach_of_points,
};

/// A render's background: a flat light grey, sRGB. A frame's is the caller's
/// ([`Frame::background`]).
pub const BACKGROUND: [u8; 3] = [230, 233, 237];
/// Fusion's default appearance (steel, satin), sRGB.
pub const BODY: [u8; 3] = [170, 175, 182];
/// Edges, sRGB.
pub const EDGE: [u8; 3] = [40, 40, 40];
/// Fusion's selection blue, sRGB.
pub const HIGHLIGHT: [u8; 3] = [6, 150, 215];
/// How much of the highlight's blue a highlighted face is tinted with.
pub const HIGHLIGHT_STRENGTH: f32 = 0.5;
/// Edges' widths in points (pixels, for the worker): every edge, dark and solid, and a
/// highlighted one.
pub const EDGE_WIDTH: f32 = 2.0;
pub const HIGHLIGHTED_EDGE_WIDTH: f32 = 4.0;
/// A highlighted face's outline, in points (pixels, for the worker).
pub const OUTLINE_WIDTH: f32 = 1.5;
/// The format of the target a view is composited into: the worker reads its image from the
/// view's own target, so this is only the render context's.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The most a frame's picking rectangle may be on a side, in pixels: Rerun's is a few points
/// about the pointer, and each of its pixels is read back.
pub const MAX_PICKING_SIDE: u32 = 256;
/// How far outside its frame a picking rectangle's corner may be, in pixels on each axis: so
/// its far corner can't overflow.
pub const MAX_PICKING_OFFSET: i32 = 1 << 24;
/// The most pixels a frame may have for each of egui's points.
pub const MAX_PIXELS_PER_POINT: f32 = 32.0;
/// How many frames' picking requests are kept waiting for their answers: they come a frame
/// or two after their frame, so more are a driver that never answers.
const MAX_PENDING_PICKS: usize = 8;
/// How far past the scene an orthographic camera sits, of the scene's reach, and at least
/// 1 mm: so nothing is cut off behind the eye.
const CLEARANCE: f64 = 0.01;
/// The depth offset edges are drawn at, so an edge on a face square to the view isn't hidden
/// by it: about 3e-6 mm in an orthographic view. Too little where a face beside an edge rises
/// toward the eye across its stroke, so each edge's strip is moved toward the eye too: a
/// drawn one by [`drawn_edge_fraction`], a picking one by [`edge_fraction`].
const EDGE_DEPTH_OFFSET: re_renderer::DepthOffset = 1;
/// How far a frame's vertices' picking discs are drawn toward the eye: past the edges, so a
/// vertex wins the pixels it shares with its faces and edges.
const VERTEX_DEPTH_OFFSET: re_renderer::DepthOffset = 2;
/// How far toward the eye a vertex's picking disc is drawn, of the view's height at the
/// vertex: the approved prototype's tolerance (viewport.js, `pickAll`'s `eps`, 1.2% of the
/// view's height), so a vertex is picked at a pixel where no face or edge there is nearer
/// than that, as the prototype's `visible` takes it. Drawn at the vertex itself, a disc facing
/// the eye is hidden by half of every face it bounds that turns toward the eye (a box's
/// corner seen from outside, by the face rising to the nearest corner).
pub const VERTEX_REACH: f64 = 0.012;
/// The most a perspective view's picking disc or picking strip is drawn toward the eye, of
/// its point's depth, whatever its reach ([`vertex_reach`]'s and [`edge_reach`]'s cap, in
/// [`reach`]): so a view as wide as 180° keeps a vertex's disc and an edge's strip ahead of
/// the eye (and of the near plane, at half the least depth anything is drawn at).
const MOST_REACH: f64 = 0.25;
/// How far toward the eye an edge's picking strip is drawn ([`edge_picks`]), in pixels of the
/// view's depth at it (N5): an edge's stroke is at its line's depth across its width, so at
/// a concave edge, whose faces both rise toward the eye away from its line, the faces hid
/// every pixel of it. Moved this far, it wins the pixel nearest its line (half a pixel off)
/// against a face rising toward the eye by up to 4 pixels of depth a pixel across it (turned
/// up to 76° from face-on), at any pixels a point. Its whole stroke, which reaches a point
/// either side of its line, it wins against a face rising by up to 2 over the stroke's
/// pixels a point: at one pixel a point, up to 2 (63°); at two, up to 1 (45°). A wall more
/// than 2 pixels of depth thick in front of it still hides it. Never more than
/// [`VERTEX_REACH`] of the view's height, so a vertex's disc, drawn past the edges' depth
/// offset, stays ahead of its edges.
const EDGE_REACH_PIXELS: f64 = 2.0;
/// How steeply a face beside a drawn edge may rise toward the eye, in pixels of the view's
/// depth a pixel across the image, with the edge's whole stroke drawn over it
/// ([`drawn_edge_fraction`]): √2, a face turned 54.7° from face-on, as each face of a part
/// square to its axes is seen from a corner (the home view), whatever the edge's direction.
const DRAWN_EDGE_SLOPE: f64 = std::f64::consts::SQRT_2;
/// How far a display mesh lies off its surface, at most, in pixels: the half a pixel the
/// core meshes a view at (its `mesh_level`), by which a concave face's chords lie nearer the
/// eye than its surface, while an edge's line lies on its curve ([`drawn_edge_fraction`]).
const MESH_SAGITTA_PIXELS: f64 = 0.5;
/// The bit a vertex's picking instance has, above its number from 1: past the u32 the ID
/// buffer's instances are (the worker's renders draw no vertices), so [`EDGE_BIT`] and the
/// ID buffer are as they were.
pub const VERTEX_BIT: u64 = 1 << 32;
/// Frames to wait for the readbacks: re_renderer delivers them in a later frame, and each
/// frame here waits for the GPU, so they come in the first or second. The bound is for a
/// driver that never delivers them.
const READBACK_FRAMES: u32 = 100;
/// How long one wait for the GPU may take: past the 30 s the core gives a render (step 3 of
/// docs/design/m4.6-render.md), which ends a stuck worker first.
const GPU_WAIT: Duration = Duration::from_secs(60);
/// The most frames [`Renderer::prepare`] refuses in a row after the GPU fails one, before it
/// tries again: about a second at 60 frames a second. The backoff doubles from 1 to this.
pub const MAX_BACKOFF_FRAMES: u32 = 64;

/// A render: the PNG, and the ID buffer if the scene asked for it.
pub struct Rendered {
    pub png: Vec<u8>,
    pub ids: Option<Vec<PixelId>>,
}

/// Why a render, a frame, or an upload failed.
#[derive(Debug)]
pub enum RenderFailure {
    /// The GPU (lavapipe's, here: the machine's memory) ran out of memory.
    OutOfMemory(String),
    /// What the renderer was given can't be drawn (a frame's size, camera, bodies,
    /// highlights, or picking, or a mesh's structure), naming the parameter at fault: the
    /// caller's, never the GPU's. Nothing was drawn or changed.
    Invalid(InvalidParameter),
    /// Anything else: the renderer's own fault.
    Failed(String),
    /// The GPU failed a recent frame, so this one wasn't tried: [`Renderer::prepare`] backs
    /// off, refusing `frames` more frames (this one among them) before it uploads and draws
    /// again. Nothing was drawn or changed. Only frames count the backoff down, so the pane
    /// must keep asking for them (egui repaints an idle window only when asked): a repaint
    /// requested for each frame refused so, or the view is never tried again.
    BackingOff { frames: u32 },
}

impl std::fmt::Display for RenderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderFailure::OutOfMemory(what) => write!(f, "out of GPU memory: {what}"),
            RenderFailure::Invalid(invalid) => f.write_str(&invalid.message),
            RenderFailure::Failed(what) => f.write_str(what),
            RenderFailure::BackingOff { frames } => write!(
                f,
                "the GPU failed a recent frame: backing off, {frames} frame(s) refused before the next try"
            ),
        }
    }
}

/// A device and its queue on `adapter`, with what `re_renderer` asks of it: the worker's
/// own. The app draws on eframe's instead.
pub fn device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    let caps = re_renderer::device_caps::DeviceCaps::from_adapter(adapter)
        .map_err(|e| format!("the adapter can't be used: {e}"))?;
    pollster::block_on(adapter.request_device(&caps.device_descriptor()))
        .map_err(|e| format!("no device: {e}"))
}

/// A body's face, edge, or vertex, by its number from 1, as the ID buffer and the elements'
/// names number them (`Face3`, `Edge12`, `Vertex5`). The ID buffer has faces and edges; a
/// frame's picking has vertices too ([`Frame::vertices`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Element {
    Face(u32),
    Edge(u32),
    Vertex(u32),
}

impl Element {
    /// The element a picking ID's instance is: a face's number from 1, an edge's with
    /// [`EDGE_BIT`], or a vertex's with [`VERTEX_BIT`]; none for anything else (0, an edge
    /// or a vertex numbered 0, or any other bit past a u32), which no instance drawn has.
    fn of_instance(instance: u64) -> Option<Element> {
        let low = u32::try_from(instance & u64::from(u32::MAX)).ok()?;
        match instance & !u64::from(u32::MAX) {
            0 if low & EDGE_BIT != 0 => {
                let edge = low & !EDGE_BIT;
                (edge > 0).then_some(Element::Edge(edge))
            }
            0 => (low > 0).then_some(Element::Face(low)),
            VERTEX_BIT => (low > 0).then_some(Element::Vertex(low)),
            _ => None,
        }
    }
}

/// A rectangle of a frame's pixels: its top-left corner, which may be outside the frame, and
/// its size, from the frame's top-left, x right and y down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub min: [i32; 2],
    pub size: [u32; 2],
}

impl PixelRect {
    /// Whether a frame takes it as its picking: from 1 to [`MAX_PICKING_SIDE`] pixels on
    /// each side, its corner within [`MAX_PICKING_OFFSET`] of the frame's top left. The one
    /// rule, for the frame's check and for a caller asking before it sends one.
    pub fn takes(&self) -> bool {
        let sides = self.size.iter().all(|s| (1..=MAX_PICKING_SIDE).contains(s));
        let corner = self
            .min
            .iter()
            .all(|c| c.unsigned_abs() <= MAX_PICKING_OFFSET.unsigned_abs());
        sides && corner
    }
}

/// One frame of the app's viewport: how it's seen, how large it is, and what's drawn.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// The caller's number for the frame, which its picking's answer echoes
    /// ([`Picking::frame`]), so a click is matched to the frame it was made on. Any value:
    /// the renderer doesn't check repeats or order. A refused frame asks nothing, so no
    /// answer carries its serial.
    pub serial: u64,
    /// The camera, as `render` takes it ([`Camera::check`]'s rules).
    pub camera: &'a Camera,
    /// The frame's size in physical pixels, each side from 1 to the device's largest
    /// texture.
    pub size_px: [u32; 2],
    /// egui's pixels per point, which edges' widths and outlines scale by: above 0, at most
    /// [`MAX_PIXELS_PER_POINT`].
    pub pixels_per_point: f32,
    /// The uploaded bodies to draw, by number, ascending, each once. The renderer moves
    /// each into the view itself, from the camera.
    pub bodies: &'a [u32],
    /// What's highlighted: a body drawn in this frame, and one of its faces or edges by its
    /// number from 1, in any order (a repeat is the same highlight). Highlights change no
    /// picking ID. Outlined in [`Outlines::selected`].
    pub highlights: &'a [(u32, Element)],
    /// What's hovered (the app's pre-selection, M5.7), as `highlights` are given, checked as
    /// they are: outlined in [`Outlines::hovered`], on the outline's second layer, so an
    /// element both highlighted and hovered shows both. Hovers change no picking ID either.
    pub hovered: &'a [(u32, Element)],
    /// The colours the highlights and the hover are outlined in: the caller's values (the
    /// app's theme), never read from it here.
    pub outlines: Outlines,
    /// The faces the app tints, each group's faces in its tint (the app's: the selection's
    /// faces, the hover's, and those of a feature hovered in its timeline or selected), as
    /// `highlights` are given and checked as they are, each a face: an edge or a vertex is
    /// refused. A face in more than one group takes the first's tint. Each is drawn in its
    /// tint by its instance's own material and tint, so a tint, which moves with the
    /// pointer, uploads no face, outlines nothing, and changes no picking ID. A face tinted
    /// and highlighted or hovered too is drawn both: which look wins is the caller's to say,
    /// by what it lists.
    pub tinted: &'a [Tinted<'a>],
    /// The pixels to read back for picking, if any: [`Renderer::take_picking`] gives what
    /// they found, a frame or two later.
    pub picking: Option<PixelRect>,
    /// Whether edges are picked (not with the selection's edge filter off, M5.7): a frame
    /// that picks then draws each edge near the picking rectangle again, into the picking
    /// layer alone, toward the eye by its reach ([`edge_picks`]), so an edge is picked along
    /// its stroke unless a face is nearer than that there, a concave edge as a convex one; the
    /// image is drawn as it is without them. With them off, an edge's stroke is picked where
    /// it's drawn (toward the eye by its drawn strip's reach, [`drawn_edge_fraction`], no more
    /// than its picking strip's), a concave edge's as a convex one's, and the faces beside it
    /// past its stroke.
    pub edges: bool,
    /// The radius, in points, within which a vertex is picked, if vertices are picked (none
    /// with the selection's vertex filter off, M5.7): a frame that picks then draws a disc
    /// of it about each vertex whose point lands within it of the picking rectangle, into
    /// the picking layer alone (invisible, writing no depth to the image), in f64 about the
    /// camera's point as edges are, toward the eye by [`VERTEX_REACH`] of the view's height
    /// and past the edges' depth offset, so the disc is the tolerance and wins the pixels it
    /// shares with its faces and edges unless one is nearer than that (a vertex before an
    /// edge before a face, with no rule of its own; a vertex hidden behind a face, never).
    /// Its picking ID is its body's and [`VERTEX_BIT`] with its number, its pick's point its
    /// own. Where it lands is [`project`]'s: in perspective, a vertex nearer the eye than
    /// [`MIN_EYE_DISTANCE`] has none, and isn't picked. Finite and above 0.
    pub vertices: Option<f32>,
    /// The colour behind the bodies, sRGB, opaque, the frame composited over the pass as it
    /// is; or none, the frame composited over what the pass already holds, so what the
    /// caller painted under it shows wherever no body is (the app's canvas, a gradient, its
    /// theme's `VIEWPORT_FRAME_BG`). There, what the frame holds is premultiplied, each pixel
    /// its colour times its coverage and its alpha the coverage (its edges are alpha
    /// blended, not drawn with alpha to coverage), and it's blended over the pass as such
    /// (`BlendWithBackground::Premultiplied`). A frame isn't translucent: a colour has no
    /// alpha to give. A render's is its own ([`BACKGROUND`]).
    pub background: Option<[u8; 3]>,
    /// Lines drawn among the bodies and picked by nothing (the app's shown sketches): each a
    /// set of strips in the document's coordinates, in its colour and width. Placed in the
    /// view in f64 each frame as edges are, moved toward the eye as a drawn edge is by the
    /// reach their width takes, and drawn after every body's edges at the edges' depth
    /// offset, so they're depth-tested among the bodies as an edge is (a line on a face shows
    /// on it whole, from any side, one behind a body is hidden by it) and drawn over an edge
    /// they coincide with. At most [`MAX_FRAME_LINE_POINTS`] points over all of them. They go into the picking layer as the background (object 0), which a pick passes
    /// over: a face under a line's stroke is found from the pixels beside it. An orthographic
    /// view's reach, which its near and far planes are set from, takes their boxes as it
    /// takes a body's, so lines with no body are drawn too.
    pub lines: &'a [Lines<'a>],
    /// Planes drawn among the bodies, translucent, and in no picking (the app's origin
    /// planes): each filled and outlined ([`Plane`]), at most [`MAX_FRAME_PLANES`]. The fill
    /// is drawn after every opaque face, tested against their depth and writing none, so a
    /// body in front of a plane hides it and a plane in front of a body tints it and hides
    /// nothing; the outline is placed as a line is. Neither goes into the picking layer: a
    /// frame's picking is the same with them as without them, pixel for pixel (the module
    /// `planes` says how). An orthographic view's reach takes their corners as it takes a
    /// line's, so a plane with no body is drawn whole; one the view is too small for is
    /// refused, naming `planes`.
    pub planes: &'a [Plane],
}

impl Frame<'_> {
    /// Why a renderer would refuse this frame by its own values, the bodies it holds read by
    /// `held` (a body's mesh by its number, none if it holds none), if it would: its pixels
    /// per point, its bodies (ascending, each once, each held), its tint, its highlights,
    /// hover, and tinted faces (each of a body it draws, numbered within its mesh's counts,
    /// each tinted one a face), its vertices' radius, its picking rectangle, its lines, and
    /// its planes.
    /// No GPU: what a frame's size may be is the device's, and its camera and what's in its
    /// view are checked as [`Renderer::prepare`] places it, after this.
    pub fn check<'m>(
        &self,
        held: impl Fn(u32) -> Option<&'m Mesh>,
    ) -> Result<(), InvalidParameter> {
        self.checked(held).map(|_| ())
    }

    /// [`check`](Self::check)'s, and what each body has highlighted, hovered, and tinted,
    /// as a renderer draws it.
    fn checked<'m>(
        &self,
        held: impl Fn(u32) -> Option<&'m Mesh>,
    ) -> Result<BTreeMap<u32, Lit>, InvalidParameter> {
        let ppp = self.pixels_per_point;
        if !(ppp.is_finite() && ppp > 0.0 && ppp <= MAX_PIXELS_PER_POINT) {
            return Err(InvalidParameter {
                parameter: "pixels_per_point",
                message: format!(
                    "pixels_per_point {ppp:?} must be above 0 and at most {MAX_PIXELS_PER_POINT}"
                ),
            });
        }
        let mut last = 0;
        for (i, &body) in self.bodies.iter().enumerate() {
            if body <= last {
                return Err(InvalidParameter {
                    parameter: "bodies",
                    message: format!(
                        "bodies[{i}], {body}, must be above the one before it ({last}): ascending, each once, from 1"
                    ),
                });
            }
            if held(body).is_none() {
                return Err(InvalidParameter {
                    parameter: "bodies",
                    message: format!("bodies[{i}], {body}, isn't uploaded"),
                });
            }
            last = body;
        }
        for group in self.tinted {
            group.tint.check()?;
        }
        let mut lit: BTreeMap<u32, Lit> = BTreeMap::new();
        // Each list, its parameter, and what its entries are called in a refusal: a tint
        // group's faces by the group's place too (`tinted[1].faces[0]`).
        let lists = [
            (
                "highlights",
                "highlights".to_owned(),
                self.highlights,
                Listed::Highlighted,
            ),
            (
                "hovered",
                "hovered".to_owned(),
                self.hovered,
                Listed::Hovered,
            ),
        ]
        .into_iter()
        .chain(self.tinted.iter().enumerate().map(|(g, group)| {
            (
                "tinted",
                format!("tinted[{g}].faces"),
                group.faces,
                Listed::Tinted(group.tint),
            )
        }));
        for (parameter, named, list, listed) in lists {
            for (i, &(body, element)) in list.iter().enumerate() {
                let Some(mesh) = self
                    .bodies
                    .binary_search(&body)
                    .ok()
                    .and_then(|_| held(body))
                else {
                    return Err(InvalidParameter {
                        parameter,
                        message: format!("{named}[{i}]'s body {body} isn't drawn in the frame"),
                    });
                };
                let (number, count, what) = match element {
                    Element::Face(n) => (n, mesh.face_triangles.len(), "faces"),
                    Element::Edge(n) => (n, mesh.edge_point_counts.len(), "edges"),
                    Element::Vertex(n) => (n, mesh.vertex_points.len(), "vertices"),
                };
                if !usize::try_from(number).is_ok_and(|n| (1..=count).contains(&n)) {
                    return Err(InvalidParameter {
                        parameter,
                        message: format!(
                            "{named}[{i}], {element:?} of body {body}, isn't one of its {count} {what} (numbered from 1)"
                        ),
                    });
                }
                let entry = lit.entry(body).or_default();
                match (listed, element) {
                    (Listed::Highlighted, Element::Face(n)) => entry.faces.push(n),
                    (Listed::Highlighted, Element::Edge(n)) => entry.edges.push(n),
                    (Listed::Hovered, Element::Face(n)) => entry.hovered_faces.push(n),
                    (Listed::Hovered, Element::Edge(n)) => entry.hovered_edges.push(n),
                    (Listed::Tinted(tint), Element::Face(n)) => {
                        entry.tinted_faces.push((n, tint));
                    }
                    (Listed::Tinted(_), _) => {
                        return Err(InvalidParameter {
                            parameter,
                            message: format!(
                                "{named}[{i}], {element:?} of body {body}, isn't a face: only faces are tinted"
                            ),
                        });
                    }
                    // A vertex is drawn over the frame by its caller (the app's dot), so
                    // it's checked, and outlined by nothing here.
                    (_, Element::Vertex(_)) => {}
                }
            }
        }
        for entry in lit.values_mut() {
            for numbers in [
                &mut entry.faces,
                &mut entry.edges,
                &mut entry.hovered_faces,
                &mut entry.hovered_edges,
            ] {
                numbers.sort_unstable();
                numbers.dedup();
            }
            // By number, each face once, in the first group's tint that lists it (a stable
            // sort keeps the groups' order among a face's).
            entry.tinted_faces.sort_by_key(|&(number, _)| number);
            entry.tinted_faces.dedup_by_key(|&mut (number, _)| number);
        }
        if let Some(radius) = self.vertices
            && !(radius.is_finite() && radius > 0.0)
        {
            return Err(InvalidParameter {
                parameter: "vertices",
                message: format!(
                    "vertices {radius:?}, a radius in points, must be finite and above 0"
                ),
            });
        }
        if let Some(rect) = self.picking
            && !rect.takes()
        {
            return Err(InvalidParameter {
                parameter: "picking",
                message: format!(
                    "picking {rect:?} must be from 1 to {MAX_PICKING_SIDE} pixels on each side, its corner within {MAX_PICKING_OFFSET} of the frame's"
                ),
            });
        }
        let points: usize = self.lines.iter().map(|set| set.strips.points()).sum();
        if points > MAX_FRAME_LINE_POINTS {
            return Err(InvalidParameter {
                parameter: "lines",
                message: format!(
                    "lines have {points} points over their sets, more than a frame's {MAX_FRAME_LINE_POINTS}"
                ),
            });
        }
        planes::check(self.planes)?;
        for (i, set) in self.lines.iter().enumerate() {
            let width = set.width;
            if !(width.is_finite() && width > 0.0 && width <= MAX_LINE_WIDTH) {
                return Err(InvalidParameter {
                    parameter: "lines",
                    message: format!(
                        "lines[{i}]'s width {width:?}, in points, must be above 0 and at most {MAX_LINE_WIDTH}"
                    ),
                });
            }
            if set.colour[3] != 255 {
                return Err(InvalidParameter {
                    parameter: "lines",
                    message: format!(
                        "lines[{i}]'s colour {:?} must be opaque, its alpha 255",
                        set.colour
                    ),
                });
            }
        }
        Ok(lit)
    }
}

/// The widest a frame's [`Lines`] may be, in points: well past an edge's widest, 4.
pub const MAX_LINE_WIDTH: f32 = 64.0;

/// The most points a frame's [`Lines`] may have over all their sets, as a render's meshes
/// may have at most `MAX_RENDER_TRIANGLES`: each is placed in f64 and uploaded every frame,
/// on the caller's thread, and past `re_renderer`'s data texture its vertices would be
/// dropped with only a log line, the edges' picking strips after them first.
pub const MAX_FRAME_LINE_POINTS: usize = 2_000_000;

/// One set of a frame's lines ([`Frame::lines`]): its strips, its colour (sRGB, opaque: its
/// alpha 255), and its width in points (above 0, at most [`MAX_LINE_WIDTH`]).
#[derive(Debug, Clone, Copy)]
pub struct Lines<'a> {
    pub strips: &'a Strips,
    pub colour: [u8; 4],
    pub width: f32,
}

/// Polylines in the document's coordinates, checked once when they're made
/// ([`Strips::new`]): each of two points or more, every coordinate finite and within
/// [`MAX_RENDER_COORDINATE`] of the origin. Each strip's box and the whole box are kept, so a
/// frame tests a box before it places what's in it.
#[derive(Clone, PartialEq, Default)]
pub struct Strips {
    points: Vec<[f64; 3]>,
    /// Where each strip ends in `points`, ascending: the `k`th is from the one before's end
    /// (0 for the first) to its own.
    ends: Vec<usize>,
    boxes: Vec<Bounds>,
    bounds: Option<Bounds>,
}

impl Strips {
    /// `polylines` as strips, in their order; refused, naming `lines` and which strip and
    /// point, if one has fewer than two points or a coordinate that isn't finite or is
    /// farther than [`MAX_RENDER_COORDINATE`] from the origin. None makes an empty set,
    /// which draws nothing.
    pub fn new(
        polylines: impl IntoIterator<Item = Vec<[f64; 3]>>,
    ) -> Result<Strips, InvalidParameter> {
        let mut strips = Strips::default();
        for (k, polyline) in polylines.into_iter().enumerate() {
            if polyline.len() < 2 {
                return Err(InvalidParameter {
                    parameter: "lines",
                    message: format!(
                        "strip {k} has {} point(s): a strip needs two or more",
                        polyline.len()
                    ),
                });
            }
            let within = |p: &[f64; 3]| p.iter().all(|c| c.abs() <= MAX_RENDER_COORDINATE);
            if let Some((i, p)) = polyline.iter().enumerate().find(|(_, p)| !within(p)) {
                return Err(InvalidParameter {
                    parameter: "lines",
                    message: format!(
                        "strip {k}'s point {i}, {p:?}, must be finite and within {MAX_RENDER_COORDINATE} mm of the origin"
                    ),
                });
            }
            // Two points or more: a box.
            let Some(bounds) = box_of(polyline.iter()) else {
                continue;
            };
            strips.bounds = Some(strips.bounds.map_or(bounds, |all| joined(all, bounds)));
            strips.points.extend(polyline);
            strips.ends.push(strips.points.len());
            strips.boxes.push(bounds);
        }
        Ok(strips)
    }

    /// The box of every point, its lowest and highest corners; none for no strips.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        self.bounds
    }

    /// How many strips there are.
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// How many points there are, over every strip.
    pub fn points(&self) -> usize {
        self.points.len()
    }

    /// The `k`th strip's points, in order; none past the last.
    pub fn strip(&self, k: usize) -> Option<&[[f64; 3]]> {
        let end = *self.ends.get(k)?;
        let start = k
            .checked_sub(1)
            .and_then(|k| self.ends.get(k))
            .copied()
            .unwrap_or(0);
        self.points.get(start..end)
    }

    /// Each strip's points, in order, and its box.
    fn strips(&self) -> impl Iterator<Item = (&[[f64; 3]], Bounds)> {
        let starts = std::iter::once(0).chain(self.ends.iter().copied());
        starts
            .zip(&self.ends)
            .zip(&self.boxes)
            .map(|((start, &end), &bounds)| (self.points.get(start..end).unwrap_or(&[]), bounds))
    }
}

impl std::fmt::Debug for Strips {
    /// Its counts and its box, not its points: a frame's refusal prints the frame.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Strips")
            .field("strips", &self.len())
            .field("points", &self.points())
            .field("bounds", &self.bounds)
            .finish()
    }
}

/// The colours a frame's outlines are drawn in, sRGB: its highlights' (a selection's) and
/// its hover's. Both are drawn [`OUTLINE_WIDTH`] wide: re_renderer's outline has one radius
/// for both its layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outlines {
    pub selected: [u8; 3],
    pub hovered: [u8; 3],
}

/// The colours [`Renderer::build`] draws a view in: its outlines', the one behind it, or
/// none, the view composited over what its pass holds.
#[derive(Clone, Copy)]
struct Colours {
    outlines: Outlines,
    background: Option<Rgba>,
}

impl Outlines {
    /// The highlight's blue, and a hover in white.
    pub const DEFAULT: Outlines = Outlines {
        selected: HIGHLIGHT,
        hovered: [255, 255, 255],
    };
}

impl Default for Outlines {
    fn default() -> Self {
        Outlines::DEFAULT
    }
}

/// The tint a frame's tinted faces are drawn in ([`Frame::tinted`]): a face's colour mixed
/// with `colour` (sRGB) at `amount` in linear light, `(1 − amount) · body + amount ·
/// colour`, then lit as any face is, as a render's highlighted face is mixed with the
/// highlight's blue ([`HIGHLIGHT_STRENGTH`]).
///
/// `amount` is finite, from 0 (no tint) to 1 (the colour alone), one of a few a caller
/// uses (each amount's material is made once and kept: a fade through many amounts would
/// keep a material for each). Drawn by the face's instance: its mesh's material scaling the
/// body's colour by `1 − amount` ([`Mixes`], the face's vertices shared, so nothing is
/// uploaded but a material for each amount, once),
/// and `re_renderer`'s per-instance tint adding `amount · colour`, opaque (one that isn't
/// makes the face translucent). So any colour mixes, lighter or darker than the body's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    pub colour: [u8; 3],
    pub amount: f32,
}

/// Faces a frame tints, and the tint they're drawn in ([`Frame::tinted`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tinted<'a> {
    pub faces: &'a [(u32, Element)],
    pub tint: Tint,
}

impl Tint {
    /// No tint: the body's colour as it is.
    pub const NONE: Tint = Tint {
        colour: BODY,
        amount: 0.0,
    };

    /// What `re_renderer` adds to a tinted face's colour, scaled by `1 − amount`, for the
    /// mix: the colour's part, `amount · colour`, in linear light. Opaque, so the face stays
    /// opaque.
    fn added(self) -> Color32 {
        let colour = srgb(self.colour);
        Color32::from(Rgba::from_rgb(
            self.amount * colour.r(),
            self.amount * colour.g(),
            self.amount * colour.b(),
        ))
    }

    /// Why a frame can't draw this tint, if it can't: an amount not finite or outside 0 to
    /// 1.
    fn check(self) -> Result<(), InvalidParameter> {
        let amount = self.amount;
        if !(amount.is_finite() && (0.0..=1.0).contains(&amount)) {
            return Err(InvalidParameter {
                parameter: "tint",
                message: format!("tint's amount {amount:?} must be finite, from 0 to 1"),
            });
        }
        Ok(())
    }
}

/// What a picking rectangle found nearest its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    /// The body's number.
    pub body: u32,
    /// Its face, edge, or vertex there.
    pub hit: Element,
    /// The pixel it was found at, in the frame.
    pub pixel: [u32; 2],
    /// The point seen at the pixel's centre, in the document's coordinates: the centre
    /// unprojected at the picking layer's depth there, in f64 (the depth itself is the GPU's
    /// f32); none if the depth wasn't read back. A vertex's is its own point. An edge's is its
    /// strip's taken back from the eye by the reach it was drawn toward it by: its picking
    /// strip's ([`edge_picks`]) in a frame that picks edges ([`Frame::edges`]), its drawn
    /// strip's ([`drawn_edge_fraction`]) in one that doesn't; the point its line is at there.
    /// (In a frame that picks edges, where the drawn stroke alone covers a pixel, at its border
    /// to f32's rounding, it would be behind by what the drawn strip's reach falls short of the
    /// picking strip's, at most 1.5 pixels of depth.)
    pub point: Option<[f64; 3]>,
}

/// A frame's picking, answered: the frame that asked (its [`Frame::serial`]), the rectangle it asked for, and what in it, of the frame's own
/// pixels, lies nearest the rectangle's centre (the first in rows, top to bottom, of those
/// as near), or none if only the background does.
///
/// A body forgotten or replaced (by an upload of another mesh) since its frame asked, up to
/// the moment the answer is taken, is stale there: its pixels are passed over as the
/// background's, so the hit is the nearest pixel of a body still drawn as it was, or none.
/// The hit is chosen when [`Renderer::take_picking`] gives it, from the readback kept till
/// then. An answer is still given, so a hover can clear: never a face of a mesh the frame
/// didn't draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picking {
    /// The serial of the frame that asked ([`Frame::serial`]).
    pub frame: u64,
    pub rect: PixelRect,
    pub hit: Option<Pick>,
    /// Of the same pixels, the edge's nearest the rectangle's centre (the first in rows of
    /// those as near), if an edge was drawn in any: an edge is a line a few pixels wide, so a
    /// pick that takes one within a tolerance band (M5.7) reads the band's square and
    /// prefers it to the face under the centre. The same rules as `hit`'s otherwise.
    pub edge: Option<Pick>,
}

/// A render context on one device, kept between renders and frames, with the bodies it
/// draws uploaded.
pub struct Renderer {
    ctx: RenderContext,
    /// The next readback's identifier (each render takes two, each frame that picks one),
    /// so a render or frame that failed never leaves a readback a later one could take as
    /// its own.
    readback: u64,
    /// The bodies, by number, as uploaded: the app's ([`Renderer::upload`]), or the last
    /// render's (a body the next render has again with the same mesh is drawn from them;
    /// the others are dropped before it uploads anything, and all of them after a render
    /// that failed).
    bodies: BTreeMap<u32, Uploaded>,
    /// How many face meshes it has uploaded.
    uploads: u64,
    /// How long the last render or frame took to place its faces and edges for its view, on
    /// the CPU.
    placed: Duration,
    /// The last frame prepared, which [`Renderer::composite`] draws until the next.
    view: Option<ViewBuilder>,
    /// The frames' picking requests waiting for their answers, by readback identifier.
    pending: BTreeMap<u64, PendingPick>,
    /// The newest picking answered and not yet taken: its request and its readback, the
    /// hit chosen from them when it's taken.
    picked: Option<Answer>,
    /// The last generation a body's mesh was given: each new mesh takes the next, so a
    /// picking answer knows a body changed since its frame.
    generations: u64,
    /// How [`Renderer::prepare`] backs off after the GPU fails a frame.
    backoff: Backoff,
    /// The planes' fill meshes, one for each fill colour drawn ([`planes::Meshes`]).
    plane_meshes: planes::Meshes,
    /// The materials a tinted face's instance is drawn with, one for each tint's amount.
    mixes: Mixes,
    /// How many of the next frames fail as the GPU fails one, once built: a stand-in for
    /// out of memory or a lost device, which lavapipe can't be made to give.
    #[cfg(test)]
    fail_frames: u32,
}

/// [`Renderer::prepare`]'s backoff after the GPU fails a frame: so a failure that would
/// come again (out of memory, a lost device) isn't met by every later frame uploading every
/// face again and failing again. After a failure the next `refuse` frames are refused at
/// once, then one is tried; each failure in a row doubles the frames refused after it, from
/// 1 to [`MAX_BACKOFF_FRAMES`]. A frame that succeeds ends it; a new mesh uploaded has the
/// next frame tried, but keeps the doubling, so a failure that persists through edits still
/// backs off to the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Backoff {
    /// How many more frames are refused.
    refuse: u32,
    /// How many the next failure refuses.
    next: u32,
}

impl Backoff {
    const NONE: Backoff = Backoff { refuse: 0, next: 1 };

    /// The next frame tried, the doubling kept: what a new mesh does.
    fn try_next(&mut self) {
        self.refuse = 0;
    }

    /// Another failure: its frames to refuse, and the next failure's doubled.
    fn failed(&mut self) {
        self.refuse = self.next;
        self.next = self.next.saturating_mul(2).min(MAX_BACKOFF_FRAMES);
    }

    /// Whether this frame is refused, and if it is, how many are refused from it on (it
    /// among them), counted down by it.
    fn refuses(&mut self) -> Option<u32> {
        (self.refuse > 0).then(|| {
            let left = self.refuse;
            self.refuse -= 1;
            left
        })
    }
}

impl Renderer {
    /// A render context on `device` (made on `adapter`, with [`device`] or as eframe makes
    /// one), compositing into targets of `output_format`.
    pub fn new(
        adapter: &wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        output_format: wgpu::TextureFormat,
    ) -> Result<Renderer, String> {
        let mut ctx = RenderContext::new(
            adapter,
            device,
            queue,
            output_format,
            RenderConfig::best_for_device_caps,
        )
        .map_err(|e| format!("no render context: {e}"))?;
        planes::register(&mut ctx);
        Ok(Renderer {
            ctx,
            readback: 1,
            bodies: BTreeMap::new(),
            uploads: 0,
            placed: Duration::ZERO,
            view: None,
            pending: BTreeMap::new(),
            picked: None,
            generations: 0,
            backoff: Backoff::NONE,
            plane_meshes: planes::Meshes::new(),
            mixes: Mixes::default(),
            #[cfg(test)]
            fail_frames: 0,
        })
    }

    /// How many face meshes it has uploaded since it was made: a face's, plain or in the
    /// highlight's tint, the first time it's drawn so, and again after its body's mesh, the
    /// point it's drawn about ([`View::anchor`]), or its highlight changed.
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    /// How long the last render or frame took to place its faces, edges, and lines for its
    /// view on the CPU (each face's anchor and transform, its instance, and the edges' and
    /// lines' strips), uploads included: for the bench, never a result.
    pub fn placed(&self) -> Duration {
        self.placed
    }

    /// `mesh` as body `body`'s (a number from 1), for frames to draw: kept until it's
    /// forgotten or replaced, and its faces uploaded to the GPU the first time a frame
    /// draws them, never again for another view (but a large face seen close,
    /// [`View::anchor`]). The same mesh again (the same `Arc`, or an equal mesh) changes
    /// nothing; another replaces the body's, so its faces are uploaded again. A mesh that
    /// fails [`check_render_mesh`], or a body numbered 0 (the background's), is refused,
    /// and the body is as it was. A new mesh taken has the next frame tried, through a
    /// backoff ([`Renderer::prepare`]), but keeps its doubling: if that frame fails too,
    /// the frames refused after it are twice the last.
    pub fn upload(&mut self, body: u32, mesh: &Arc<Mesh>) -> Result<(), RenderFailure> {
        let _span =
            tracing::debug_span!("upload_body", body, faces = mesh.face_triangles.len()).entered();
        if body == 0 {
            return Err(invalid(
                "body",
                "body 0 isn't a body's number: they're from 1 (0 is the background's)".to_owned(),
            ));
        }
        if let Some(held) = self.bodies.get_mut(&body)
            && (Arc::ptr_eq(&held.mesh, mesh) || *held.mesh == **mesh)
        {
            held.mesh = Arc::clone(mesh);
            return Ok(());
        }
        check_render_mesh(mesh)
            .map_err(|what| invalid("mesh", format!("body {body}'s mesh: {what}")))?;
        let mut uploaded = Uploaded::new(Arc::clone(mesh)).map_err(RenderFailure::Failed)?;
        uploaded.generation = self.next_generation();
        self.bodies.insert(body, uploaded);
        self.backoff.try_next();
        Ok(())
    }

    /// Body `body` dropped, and its faces' GPU meshes with it; nothing if it isn't
    /// uploaded. A picking answer still to come passes over it ([`Picking`]).
    pub fn forget(&mut self, body: u32) {
        self.bodies.remove(&body);
    }

    fn next_generation(&mut self) -> u64 {
        self.generations += 1;
        self.generations
    }

    /// `frame` built and drawn into `re_renderer`'s own target, never waiting for the GPU:
    /// the command buffer to submit (egui_wgpu's `prepare` returns it), which
    /// [`Renderer::composite`] then composites. Each body's faces are moved into the view
    /// and its edges placed in it in f64, as [`Renderer::render`] draws them; each
    /// highlighted face's and edge's instance is outlined in [`Outlines::selected`], and each
    /// hovered one in [`Outlines::hovered`], its mesh and colour as uploaded, so neither
    /// uploads anything ([`Highlight::Outline`]); the picking
    /// rectangle, if any, is read back for [`Renderer::take_picking`]. One frame for each of
    /// egui's frames: it begins `re_renderer`'s frame.
    ///
    /// A frame has no cap on its faces, edges, or triangles: [`RenderScene::check`]'s are the
    /// worker's, for a render's time; a frame draws what the model has. A picking answer comes
    /// only with later frames (re_renderer reads it back when the next frame begins), so the
    /// pane must keep repainting while one is due.
    ///
    /// A frame that fails its checks is refused ([`RenderFailure::Invalid`]) before anything
    /// is done: it asks for no picking. A frame the GPU fails (out of memory, or anything
    /// wgpu finds invalid) keeps the bodies but none of their GPU meshes, which the next
    /// frame uploads again, and its picking request is dropped. Either way nothing is
    /// composited until a frame succeeds.
    ///
    /// After a frame the GPU failed it backs off, so a failure that would come again isn't
    /// met by every frame uploading every face again: the next frame is refused at once
    /// ([`RenderFailure::BackingOff`], before its checks, asking for no picking), then one is
    /// tried; each failure in a row doubles the frames refused after it, 1, 2, 4, … up to
    /// [`MAX_BACKOFF_FRAMES`]. A frame that succeeds ends it; a new mesh uploaded has the
    /// next frame tried, its doubling kept. [`Renderer::forget`] changes nothing of it. The
    /// pane must keep asking for frames while it backs off ([`RenderFailure::BackingOff`]).
    pub fn prepare(&mut self, frame: &Frame<'_>) -> Result<wgpu::CommandBuffer, RenderFailure> {
        let _span = tracing::debug_span!(
            "prepare",
            bodies = frame.bodies.len(),
            width = frame.size_px[0],
            height = frame.size_px[1],
            picking = frame.picking.is_some()
        )
        .entered();
        self.view = None;
        if let Some(frames) = self.backoff.refuses() {
            return Err(RenderFailure::BackingOff { frames });
        }
        let CheckedFrame { view, lit, planes } =
            self.check_frame(frame).map_err(RenderFailure::Invalid)?;
        let device = self.ctx.device.clone();
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validity = device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.ctx.begin_frame();
        self.harvest();
        let picking = frame.picking.map(|rect| {
            let id = self.readback;
            self.readback += 1;
            self.pending.insert(
                id,
                PendingPick {
                    serial: frame.serial,
                    bodies: frame
                        .bodies
                        .iter()
                        .filter_map(|b| self.bodies.get(b).map(|u| (*b, u.generation)))
                        .collect(),
                    rect,
                    size: frame.size_px,
                    frame: view.frame,
                    origin: view.origin,
                    depth: view.depth,
                    projection: view.projection,
                    perspective: view.perspective,
                    span: view.span,
                    strip_fraction: if frame.edges {
                        edge_fraction(view.height)
                    } else {
                        drawn_edge_fraction(view.height, frame.pixels_per_point)
                    },
                },
            );
            (rect, id)
        });
        let plain = Lit::default();
        let drawn: Vec<(u32, &Lit)> = frame
            .bodies
            .iter()
            .map(|&b| (b, lit.get(&b).unwrap_or(&plain)))
            .collect();
        let built = self.build(
            &view,
            frame.pixels_per_point,
            Colours {
                outlines: frame.outlines,
                background: frame.background.map(srgb),
            },
            Drawn {
                bodies: &drawn,
                lines: frame.lines,
                planes: &planes,
            },
            picking.map(|(rect, id)| Picked {
                rect,
                id,
                vertices: frame.vertices,
                edges: frame.edges,
            }),
            None,
        );
        #[cfg(test)]
        let built = match self.fail_frames {
            0 => built,
            _ => {
                self.fail_frames -= 1;
                Err("a stand-in for the GPU's failure".to_owned())
            }
        };
        self.ctx.before_submit();
        // On wgpu's native backends these are answered at once: no wait.
        let invalid = pollster::block_on(validity.pop());
        let short = pollster::block_on(memory.pop());
        match scoped(short, invalid, built) {
            Ok((commands, builder)) => {
                self.view = Some(builder);
                self.backoff = Backoff::NONE;
                Ok(commands)
            }
            Err(failure) => {
                self.failed(picking.map(|(_, id)| id));
                Err(failure)
            }
        }
    }

    /// What a frame the GPU failed leaves: its picking request (`request`, its readback's
    /// identifier) dropped, and every body's GPU meshes, the planes', and the tints'
    /// materials, which nothing vouches for, to be made again when they're next drawn; the
    /// bodies themselves kept; and the next frames refused for a while ([`Backoff`]).
    fn failed(&mut self, request: Option<u64>) {
        if let Some(id) = request {
            self.pending.remove(&id);
        }
        self.backoff.failed();
        for body in self.bodies.values_mut() {
            body.drop_gpu_meshes();
        }
        self.plane_meshes.clear();
        self.mixes.donors.clear();
    }

    /// The last frame [`Renderer::prepare`] built, composited into `pass` (egui's, in its
    /// `paint`, its viewport the callback's rectangle; the pass's target in the format the
    /// renderer was made with). Nothing, if no frame was prepared or the last one failed.
    pub fn composite(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(view) = &self.view {
            view.composite(&self.ctx, pass);
        }
    }

    /// The newest frame's picking the GPU has answered and that wasn't taken yet, never
    /// waiting: a frame or two after the frame that asked for it (none before, and none
    /// again once taken). An answer older than one already taken is never given.
    pub fn take_picking(&mut self) -> Option<Picking> {
        self.harvest();
        let Answer {
            request,
            ids,
            depths,
            extent,
        } = self.picked.take()?;
        // Drawn in its frame, and as it is now, when it's taken: neither forgotten nor
        // replaced since.
        let current = |body: u32| {
            let drawn = request
                .bodies
                .binary_search_by_key(&body, |&(b, _)| b)
                .ok()
                .and_then(|i| request.bodies.get(i))
                .map(|&(_, generation)| generation);
            drawn.is_some() && drawn == self.bodies.get(&body).map(|u| u.generation)
        };
        let [any, edge] = request.nearest(&ids, extent, current);
        let bodies = &self.bodies;
        let with_point = |(i, mut pick): (u32, Pick)| {
            if let Element::Vertex(n) = pick.hit {
                // A vertex's own point: its disc is drawn toward the eye ([`VERTEX_REACH`]).
                pick.point = bodies.get(&pick.body).and_then(|body| {
                    let at = usize::try_from(n).ok()?.checked_sub(1)?;
                    body.mesh.vertex_points.get(at).copied()
                });
            } else if depths.len() == ids.len()
                && let Some(&depth) = usize::try_from(i).ok().and_then(|i| depths.get(i))
            {
                // An edge's strip is drawn toward the eye (its picking strip, [`edge_picks`],
                // or, with edges not picked, its drawn strip): its point is taken back by as
                // much.
                let edge = matches!(pick.hit, Element::Edge(_));
                pick.point = request.point_at(pick.pixel, depth, edge);
            }
            pick
        };
        Some(Picking {
            frame: request.serial,
            rect: request.rect,
            hit: any.map(with_point),
            edge: edge.map(with_point),
        })
    }

    /// `frame`'s checks, and what it draws: its view, each body's highlights, and each plane
    /// with its outline.
    fn check_frame(&self, frame: &Frame<'_>) -> Result<CheckedFrame, InvalidParameter> {
        let largest = self.ctx.device.limits().max_texture_dimension_2d;
        if frame
            .size_px
            .iter()
            .any(|side| !(1..=largest).contains(side))
        {
            return Err(InvalidParameter {
                parameter: "size_px",
                message: format!(
                    "size_px {:?} must be from 1 to {largest} pixels on each side",
                    frame.size_px
                ),
            });
        }
        let lit = frame.checked(|body| self.bodies.get(&body).map(|uploaded| &*uploaded.mesh))?;
        // The camera at this size, and the bodies and lines in its view, by the rules a
        // scene's check takes (`RenderScene::check`).
        let [_, height] = frame.size_px;
        let camera_frame = frame.camera.check(frame.size_px[0], height)?;
        let held = &self.bodies;
        let drawn = frame.bodies.iter().filter_map(|b| {
            held.get(b).map(|u| InViewBody {
                body: *b,
                mesh: &u.mesh,
                bounds: u.bounds,
            })
        });
        let target = frame.camera.target;
        let mut exact = |_: u32, mesh: &Mesh| reach_of_points(mesh, target);
        let in_view = check_bodies_in_view(frame.camera, height, drawn, &mut exact)?;
        let in_view = with_lines(
            frame.camera,
            height,
            in_view,
            frame.lines.iter().map(|set| set.strips),
            "lines",
        )?;
        // Each checked by `checked`: an outline each.
        let planes: Vec<(Plane, Strips)> = frame
            .planes
            .iter()
            .filter_map(|plane| plane.outline().map(|outline| (*plane, outline)))
            .collect();
        let in_view = with_lines(
            frame.camera,
            height,
            in_view,
            planes.iter().map(|(_, outline)| outline),
            "planes",
        )?;
        let view = View::new(frame.camera, camera_frame, frame.size_px, in_view);
        Ok(CheckedFrame { view, lit, planes })
    }

    /// The newest picking the GPU has answered, kept for [`Renderer::take_picking`] (which
    /// chooses its hit from it then, against the bodies as they are when it's taken), and
    /// every request older than it dropped; never waiting.
    fn harvest(&mut self) {
        if let Err(e) = self.ctx.device.poll(wgpu::PollType::Poll) {
            tracing::debug!(error = %e, "the device couldn't be polled");
        }
        let mut newest = None;
        for &id in self.pending.keys() {
            if let Some(result) = PickingLayerProcessor::readback_result(&self.ctx, id) {
                newest = Some((id, result));
            }
        }
        if let Some((id, result)) = newest {
            // It, and the requests after it; those before it are dropped.
            let mut newer = self.pending.split_off(&id);
            if let Some(request) = newer.remove(&id) {
                self.picked = Some(Answer {
                    request,
                    ids: result.picking_id_data,
                    depths: result.picking_depth_data,
                    extent: result.rect.extent,
                });
            }
            self.pending = newer;
        }
        while self.pending.len() > MAX_PENDING_PICKS {
            self.pending.pop_first();
        }
    }

    /// `scene` drawn. The scene must have passed [`RenderScene::check`]. Errors the GPU
    /// reports while it draws (out of memory, or anything wgpu finds invalid) fail the
    /// render rather than leave a blank image. The renderer keeps only the scene's bodies:
    /// it's the worker's, or the bench's, not a viewport's. Don't call it on a renderer that
    /// holds the app's uploads ([`Renderer::upload`]): it drops every body the scene hasn't.
    pub fn render(&mut self, scene: &RenderScene) -> Result<Rendered, RenderFailure> {
        let _span = tracing::debug_span!(
            "render",
            bodies = scene.bodies.len(),
            width = scene.width,
            height = scene.height
        )
        .entered();
        self.view = None;
        let device = self.ctx.device.clone();
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validity = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let ids = (self.readback, self.readback + 1);
        self.readback += 2;
        let drawn = self.draw(scene, ids);
        let invalid = pollster::block_on(validity.pop());
        let short = pollster::block_on(memory.pop());
        let result = scoped(short, invalid, drawn);
        if result.is_err() {
            // Nothing vouches for buffers a failed render made: none is drawn again.
            self.bodies.clear();
        }
        result
    }

    /// `scene` drawn, its readbacks identified by `(picking, screenshot)`: built as a
    /// frame is, then submitted and waited for.
    fn draw(
        &mut self,
        scene: &RenderScene,
        (picking, screenshot): (u64, u64),
    ) -> Result<Rendered, String> {
        let view = View::of(&scene.camera, scene)?;
        // What this scene doesn't draw again is dropped before anything is uploaded: the
        // bodies it hasn't with the same mesh (and of those it has, [`Uploaded::settle`]
        // drops each face's mesh about another point or in the other colour). So the GPU
        // holds about one scene's meshes.
        self.bodies.retain(|number, uploaded| {
            scene
                .bodies
                .binary_search_by_key(number, |b| b.body)
                .is_ok_and(|i| *uploaded.mesh == scene.bodies[i].mesh)
        });
        let new: Vec<&RenderBody> = scene
            .bodies
            .iter()
            .filter(|b| !self.bodies.contains_key(&b.body))
            .collect();
        for body in new {
            let mut uploaded = Uploaded::new(Arc::new(body.mesh.clone()))?;
            uploaded.generation = self.next_generation();
            self.bodies.insert(body.body, uploaded);
        }
        let lit: Vec<Lit> = scene.bodies.iter().map(Lit::of_body).collect();
        let drawn: Vec<(u32, &Lit)> = scene.bodies.iter().map(|b| b.body).zip(&lit).collect();
        let whole = PixelRect {
            min: [0, 0],
            size: [scene.width, scene.height],
        };
        self.ctx.begin_frame();
        let built = self.build(
            &view,
            1.0,
            Colours {
                outlines: Outlines::DEFAULT,
                background: Some(srgb(BACKGROUND)),
            },
            Drawn {
                bodies: &drawn,
                lines: &[],
                planes: &[],
            },
            scene.ids.then_some(Picked {
                rect: whole,
                id: picking,
                vertices: None,
                edges: false,
            }),
            Some(screenshot),
        );
        self.ctx.before_submit();
        let (commands, _view) = built?;
        self.ctx.queue.submit([commands]);
        read_back(&mut self.ctx, scene, picking, screenshot)
    }

    /// The view `view` of what's `drawn`, its bodies (each uploaded, with its highlights) and
    /// its lines, in a
    /// frame `re_renderer`'s already begun: each face an instance moved into the view, its
    /// mesh uploaded where it has none; each edge a strip placed in the view; then each
    /// line's strip placed in it ([`lines_batch`]); its planes, in no picking ([`planes::draw`]);
    /// highlights
    /// drawn in each body's [`Lit::style`], and outlined in `colours`' outlines; the picking
    /// rectangle and its readback's identifier, with the vertices' discs it asks for
    /// ([`vertices`]), and the image's readback, if asked; behind it all, `colours`'
    /// background, or none, the view composited over what its pass holds. The command buffer
    /// that draws it into the view's target, and the view, to composite.
    fn build(
        &mut self,
        view: &View,
        pixels_per_point: f32,
        colours: Colours,
        drawn: Drawn<'_>,
        picking: Option<Picked>,
        screenshot: Option<u64>,
    ) -> Result<(wgpu::CommandBuffer, ViewBuilder), String> {
        let Colours {
            outlines,
            background,
        } = colours;
        let Drawn {
            bodies: drawn,
            lines: sets,
            planes,
        } = drawn;
        let Renderer {
            ctx,
            bodies,
            uploads,
            placed,
            plane_meshes,
            mixes,
            ..
        } = self;
        let placing = std::time::Instant::now();
        let mut instances = Vec::new();
        let mut lines = LineDrawableBuilder::new(ctx);
        lines.enable_alpha_blending();
        // An edge's stroke reaches half its width past its line, so edges are placed by the
        // frustum widened by it: a plain one's, and a highlighted one's (the wider).
        let strokes = [
            view.stroke_frustum(EDGE_WIDTH, pixels_per_point),
            view.stroke_frustum(HIGHLIGHTED_EDGE_WIDTH, pixels_per_point),
        ];
        // Every drawn edge toward the eye by one reach, its plain stroke's.
        let reach = drawn_edge_fraction(view.height, pixels_per_point);
        for &(number, lit) in drawn {
            let uploaded = bodies
                .get_mut(&number)
                .ok_or_else(|| format!("body {number} isn't uploaded"))?;
            // A body that can have no pixel, by its edges' widest stroke, isn't placed: no
            // face, no edge.
            if uploaded
                .bounds
                .is_none_or(|bounds| strokes[1].class_of_box(bounds) == Class::Out)
            {
                for face in &mut uploaded.faces {
                    face.placed = false;
                }
                continue;
            }
            uploaded.settle(view, lit);
            *uploads += uploaded.faces(ctx, number, lit, mixes, view, &mut instances)?;
            edges(&mut lines, number, uploaded, lit, view, (&strokes, reach));
        }
        // The frame's lines after every body's edges, so a line coinciding with an edge is
        // drawn over it.
        let mut strips = 0usize;
        for (i, set) in sets.iter().enumerate() {
            strips += lines_batch(&mut lines, i, set, view, pixels_per_point);
        }
        if !sets.is_empty() {
            tracing::debug!(sets = sets.len(), strips, "lines");
        }
        // The edges near a frame's picking again, toward the eye, after every edge drawn (so
        // none drawn is ever left out for them, should the strips reach re_renderer's most),
        // for a picking that asks for them: a frame that doesn't pick, one whose edges aren't
        // picked, and a render, pay nothing for them.
        if let Some(Picked {
            rect, edges: true, ..
        }) = picking
            && let Some(projector) = Projector::new(&view.camera, [view.width, view.height])
        {
            let half = f64::from(EDGE_WIDTH) / 2.0 * f64::from(pixels_per_point);
            let near = Near::of(rect, half).sides(&projector);
            let mut strips = 0usize;
            for &(number, _) in drawn {
                if let Some(uploaded) = bodies.get(&number) {
                    strips += edge_picks(&mut lines, number, uploaded, view, &near, &strokes[0]);
                }
            }
            tracing::debug!(strips, "edge picks");
        }
        // The vertices' discs, only for a picking that asks for them: a frame that doesn't
        // pick pays nothing for them.
        let points = match picking {
            Some(Picked {
                rect,
                vertices: Some(radius),
                ..
            }) => {
                let mut points = PointCloudBuilder::new(ctx);
                let near = Near::of(rect, f64::from(radius) * f64::from(pixels_per_point));
                let mut discs = 0usize;
                for &(number, _) in drawn {
                    if let Some(uploaded) = bodies.get(&number) {
                        discs += vertices(&mut points, number, &uploaded.mesh, view, near, radius);
                    }
                }
                (discs > 0).then_some(points)
            }
            _ => None,
        };
        let plane_draws = if planes.is_empty() {
            Vec::new()
        } else {
            let (drawn, draws) = planes::draw(ctx, plane_meshes, planes, view, pixels_per_point)?;
            tracing::debug!(planes = planes.len(), drawn, "planes");
            draws
        };
        *placed = placing.elapsed();
        let highlighted = drawn.iter().any(|(_, lit)| lit.any());
        let config = TargetConfiguration {
            // Its name is only GPU objects' labels: the default's ("default view").
            resolution_in_pixel: [view.width, view.height],
            // The view is the default, the identity: the camera at the world's origin, looking
            // along −z with +y up. The camera reaches the GPU through each face's
            // `world_from_mesh` and the edges' placement, never through the view
            // (`a_render_takes_its_view_from_its_camera`).
            projection_from_view: view.projection,
            pixels_per_point,
            outline_config: highlighted.then(|| OutlineConfig {
                outline_radius_pixel: OUTLINE_WIDTH * pixels_per_point,
                color_layer_a: srgb(outlines.selected),
                color_layer_b: srgb(outlines.hovered),
            }),
            picking_config: picking.map(|picked| ViewPickingConfiguration {
                picking_rect: rect_int(picked.rect),
                readback_identifier: picked.id,
                show_debug_view: false,
            }),
            // With no background of its own, over what the pass holds. What the view holds is
            // premultiplied already: its faces drawn over none (0, 0, 0, 0), its edges alpha
            // blended (`enable_alpha_blending`), its vertices' discs premultiplied
            // (`FLAG_PREMULTIPLIED_ALPHA`), none by alpha to coverage. So it's blended as it
            // is: re_renderer's alpha-to-coverage blend multiplies by alpha again, leaving
            // each pixel an edge partly covers its colour times its coverage twice, a dark
            // fringe along it.
            blend_with_background: match background {
                Some(_) => BlendWithBackground::No,
                None => BlendWithBackground::Premultiplied,
            },
            ..Default::default()
        };
        let mut builder = ViewBuilder::new(ctx, config, ViewBuilderId::new(1))
            .map_err(|e| format!("no view: {e}"))?;
        if !instances.is_empty() {
            let draw = MeshDrawData::new(ctx, &instances)
                .map_err(|e| format!("no draw data for the faces: {e}"))?;
            builder
                .queue_draw(ctx, draw)
                .map_err(|e| format!("couldn't queue the faces: {e}"))?;
        }
        if !lines.is_empty() {
            let draw = lines
                .into_draw_data()
                .map_err(|e| format!("no draw data for the edges: {e}"))?;
            builder
                .queue_draw(ctx, draw)
                .map_err(|e| format!("couldn't queue the edges: {e}"))?;
        } else {
            // Its borrow of the context ends here.
            drop(lines);
        }
        if let Some(points) = points {
            let draw = points
                .into_draw_data()
                .map_err(|e| format!("no draw data for the vertices: {e}"))?;
            builder
                .queue_draw(ctx, draw)
                .map_err(|e| format!("couldn't queue the vertices: {e}"))?;
        }
        // After everything else, as the planes' module says: their fills drawn last of what's
        // transparent, and neither in the picking layer.
        for draw in plane_draws {
            builder
                .queue_draw(ctx, draw)
                .map_err(|e| format!("couldn't queue the planes: {e}"))?;
        }
        if let Some(screenshot) = screenshot {
            builder
                .schedule_screenshot(ctx, screenshot, ())
                .map_err(|e| format!("couldn't schedule the image's readback: {e}"))?;
        }
        let commands = builder
            .draw(ctx, background.unwrap_or(Rgba::TRANSPARENT))
            .map_err(|e| format!("couldn't draw: {e}"))?;
        Ok((commands, builder))
    }
}

/// `in_view`, a view as its bodies' check found it ([`check_bodies_in_view`]), with `sets`,
/// a frame's lines, in it too: an orthographic view's reach takes each set's box by its
/// farthest corner ([`reach_of_box`]), and where the rule refuses the corner, its points, as a
/// body's is taken; a set whose points the rule refuses is refused, naming `lines`, since the
/// bodies alone passed. A perspective view takes any: what it draws is cut to its pyramid. So
/// the near and far planes an orthographic view is drawn between hold the lines, and a sketch
/// with no body is drawn.
fn with_lines<'a>(
    camera: &Camera,
    height: u32,
    in_view: InView,
    sets: impl IntoIterator<Item = &'a Strips>,
    parameter: &'static str,
) -> Result<InView, InvalidParameter> {
    let InView::Orthographic {
        view_height,
        mut reach,
    } = in_view
    else {
        return Ok(in_view);
    };
    let target = camera.target;
    let takes = |reach: f64| view_height >= least_view_height(height, reach);
    for (i, set) in sets.into_iter().enumerate() {
        let Some(bounds) = set.bounds else {
            continue;
        };
        let corner = reach_of_box(bounds, target);
        let this = if takes(corner) {
            corner
        } else {
            let points = set
                .points
                .iter()
                .map(|p| {
                    let d = sub(*p, target);
                    dot(d, d).sqrt()
                })
                .fold(0.0, f64::max);
            if !takes(points) {
                return Err(InvalidParameter {
                    parameter,
                    message: format!(
                        "{parameter}[{i}] reach {points:?} mm from camera.target, too far for camera.view_height {view_height:?} in an image {height} pixels high: it must be at least {:?} mm, where f32 draws them to an eighth of a pixel (the image's height times the reach over 2^19), or camera.target nearer them",
                        least_view_height(height, points)
                    ),
                });
            }
            points
        };
        reach = reach.max(this);
    }
    Ok(InView::Orthographic { view_height, reach })
}

/// What the GPU's error scopes found, or what drawing returned: out of memory first, then
/// anything invalid, then drawing's own error.
fn scoped<T>(
    short: Option<wgpu::Error>,
    invalid: Option<wgpu::Error>,
    drawn: Result<T, String>,
) -> Result<T, RenderFailure> {
    if let Some(e) = short {
        Err(RenderFailure::OutOfMemory(e.to_string()))
    } else if let Some(e) = invalid {
        Err(RenderFailure::Failed(format!(
            "the GPU found the render invalid: {e}"
        )))
    } else {
        drawn.map_err(RenderFailure::Failed)
    }
}

fn invalid(parameter: &'static str, message: String) -> RenderFailure {
    RenderFailure::Invalid(InvalidParameter { parameter, message })
}

fn rect_int(rect: PixelRect) -> RectInt {
    RectInt {
        min: glam::IVec2::new(rect.min[0], rect.min[1]),
        extent: glam::UVec2::new(rect.size[0], rect.size[1]),
    }
}

/// How highlights are drawn. The worker's renders bake the highlight's blue into a
/// highlighted face's mesh (uploaded mixed with it) and outline it, and draw a highlighted
/// edge wider and blue: the agents' PNG look (docs/design/m4.6-render.md). The app's frames
/// outline each highlighted face's and edge's instance alone, with `re_renderer`'s
/// per-instance outline masks, as Rerun's viewer draws hover and selection, and leave its
/// mesh and colour as uploaded: so a highlight, which moves with the pointer, never uploads
/// anything, and changes no pixel of the picking layer (docs/design/m5.0-app.md section 10,
/// "outlines by instance").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Highlight {
    Baked,
    /// A frame's: and a body's with no highlights, which looks the same either way.
    #[default]
    Outline,
}

/// The outline mask of an instance highlighted or not, hovered or not: the first layer for
/// a highlight, the second for a hover, both for both; none for neither.
fn outlined_as(highlighted: bool, hovered: bool) -> OutlineMaskPreference {
    if highlighted || hovered {
        OutlineMaskPreference::some(u8::from(highlighted), u8::from(hovered))
    } else {
        OutlineMaskPreference::NONE
    }
}

/// Which of a frame's lists an element is in ([`Frame::highlights`], [`Frame::hovered`],
/// [`Frame::tinted`]).
#[derive(Clone, Copy)]
enum Listed {
    Highlighted,
    Hovered,
    Tinted(Tint),
}

/// A frame as [`Renderer::check_frame`] passed it: its view, each body's highlights, and
/// each plane with its outline.
struct CheckedFrame {
    view: View,
    lit: BTreeMap<u32, Lit>,
    planes: Vec<(Plane, Strips)>,
}

/// What a view draws: its bodies, each by number with its highlights, and its lines
/// ([`Frame::lines`]; a render's, none).
#[derive(Clone, Copy)]
struct Drawn<'a> {
    bodies: &'a [(u32, &'a Lit)],
    lines: &'a [Lines<'a>],
    /// Each plane with its outline ([`Frame::planes`]; a render's, none).
    planes: &'a [(Plane, Strips)],
}

/// What a view's picking asks: its rectangle, its readback's identifier, the radius in
/// points its vertices are picked within, if they are ([`Frame::vertices`]), and whether its
/// edges are drawn again at their reach ([`edge_picks`]): a frame's are where it picks edges
/// ([`Frame::edges`]), a render's never (its ID buffer has each edge where its image draws it).
#[derive(Debug, Clone, Copy)]
struct Picked {
    rect: PixelRect,
    id: u64,
    vertices: Option<f32>,
    edges: bool,
}

/// The pixels about a picking rectangle a vertex's disc, or an edge's stroke, can reach it
/// from: the rectangle (from its corner, `size` pixels on), each side out by the disc's
/// radius (or the stroke's half width) in pixels and one pixel more (a disc's edge is
/// smoothed over about a pixel).
#[derive(Debug, Clone, Copy)]
struct Near {
    low: [f64; 2],
    high: [f64; 2],
}

impl Near {
    fn of(rect: PixelRect, radius_px: f64) -> Near {
        let out = radius_px + 1.0;
        Near {
            low: std::array::from_fn(|k| f64::from(rect.min[k]) - out),
            high: std::array::from_fn(|k| f64::from(rect.min[k]) + f64::from(rect.size[k]) + out),
        }
    }

    fn holds(&self, pixel: [f64; 2]) -> bool {
        (0..2).all(|k| self.low[k] <= pixel[k] && pixel[k] <= self.high[k])
    }

    /// Its four sides through `projector`, made once a frame, for [`NearSides::meets_box`].
    fn sides(&self, projector: &Projector) -> NearSides {
        let m = projector.matrix;
        let (x, y, w) = (m.row(0), m.row(1), m.row(3));
        let [width, height] = projector.size;
        // A point's pixel is ((x/w + 1)/2 width, (1 − y/w)/2 height) of its clip coordinates,
        // so with w above 0 (orthographic, 1; in perspective, its depth, which is so wherever
        // it's drawn), each side is a linear form of them: x − (2 low/width − 1) w ≥ 0 for the
        // left, and so on.
        let left = 2.0 * self.low[0] / width - 1.0;
        let right = 2.0 * self.high[0] / width - 1.0;
        let top = 1.0 - 2.0 * self.low[1] / height;
        let bottom = 1.0 - 2.0 * self.high[1] / height;
        let f = &projector.frame;
        let form = |q: glam::DVec4| {
            // Of the point's view coordinates (right · d, up · d, back · d, 1), d its offset
            // from the projector's origin: so of d itself.
            let normal: [f64; 3] =
                std::array::from_fn(|k| q.x * f.right[k] + q.y * f.up[k] + q.z * f.back[k]);
            (normal, q.w)
        };
        NearSides {
            origin: projector.origin,
            forms: [
                form(x - left * w),
                form(right * w - x),
                form(top * w - y),
                form(y - bottom * w),
            ],
        }
    }
}

/// A [`Near`]'s four sides, each a linear form of a point's offset from a [`Projector`]'s
/// origin (its normal, and its value there), at least 0 for a point that, drawn, lands on its
/// side of that side: the projector's own matrix, so a side is where its pixels are.
struct NearSides {
    origin: [f64; 3],
    forms: [([f64; 3], f64); 4],
}

impl NearSides {
    /// Whether anything drawn in the box `bounds` can land in the rectangle: unless the box
    /// lies wholly outside one side, its corner farthest along that side's form (a linear
    /// form's most over a box is at the corner taking each axis's end its normal points to)
    /// below 0. One corner a side, against the eight a box's pixels take; and the same test,
    /// for a box drawn whole, as whether its corners' pixels' box meets the rectangle (a
    /// projection keeps lines straight, so a box's image is within its corners'). In
    /// perspective a point nearer the eye than [`MIN_EYE_DISTANCE`] isn't drawn, so a box
    /// reaching behind the eye is judged by what of it is drawn; a value past f64 (NaN) never
    /// says it's outside.
    fn meets_box(&self, (low, high): Bounds) -> bool {
        self.forms.iter().all(|(normal, value)| {
            let corner: [f64; 3] =
                std::array::from_fn(|k| if normal[k] >= 0.0 { high[k] } else { low[k] });
            let most = dot(*normal, sub(corner, self.origin)) + value;
            most.is_nan() || most >= 0.0
        })
    }
}

/// A picking request answered: the request, and its readback's IDs and depths, `extent` in
/// size (at most [`MAX_PICKING_SIDE`]² each).
struct Answer {
    request: PendingPick,
    ids: Vec<PickingLayerId>,
    depths: Vec<f32>,
    extent: glam::UVec2,
}

/// A frame's picking request, waiting for its answer: its rectangle, the frame's size, and
/// the view it was drawn in, to take its depth back to the document.
struct PendingPick {
    /// The frame's serial, the caller's.
    serial: u64,
    /// The bodies it drew, ascending, and their meshes' generations.
    bodies: Vec<(u32, u64)>,
    rect: PixelRect,
    size: [u32; 2],
    frame: CameraFrame,
    origin: [f64; 3],
    depth: f64,
    projection: ViewProjection,
    /// The view's kind and its `span` ([`View::span`]), to take an edge's pick back by its
    /// reach ([`edge_reach`]).
    perspective: bool,
    span: f64,
    /// How far toward the eye, of the view's height, the edges' strips it picks from were
    /// moved ([`toward_eye`]): in a frame that picks edges their picking strips', an edge's
    /// reach ([`edge_fraction`]); in one that doesn't, their drawn strips' alone
    /// ([`drawn_edge_fraction`]).
    strip_fraction: f64,
}

impl PendingPick {
    /// Of the readback's pixels `ids`, `extent` in size: of the frame's own (outside the
    /// frame, re_renderer draws on as if it were larger, which isn't shown), the one with a
    /// body nearest the rectangle's centre, the first in rows of those as near, of the
    /// bodies `current` takes (those not, the background's); and, the same way, the one
    /// with an edge. Each its index in `ids`, and what's there, without its point.
    fn nearest(
        &self,
        ids: &[PickingLayerId],
        extent: glam::UVec2,
        current: impl Fn(u32) -> bool,
    ) -> [Option<(u32, Pick)>; 2] {
        let width = extent.x;
        let pixels = u64::from(width) * u64::from(extent.y);
        if ids.len() as u64 != pixels || width == 0 {
            return [None, None];
        }
        let centre = self.rect.size.map(|s| f64::from(s) / 2.0);
        let mut nearest: Option<(f64, u32, Pick)> = None;
        let mut edge: Option<(f64, u32, Pick)> = None;
        for (i, id) in ids.iter().enumerate() {
            let Ok(body) = u32::try_from(id.object.0) else {
                continue;
            };
            let Some(hit) = Element::of_instance(id.instance.0) else {
                continue;
            };
            if body == 0 || !current(body) {
                continue;
            }
            // At most MAX_PICKING_SIDE² pixels: each index a u32.
            let Ok(i) = u32::try_from(i) else { break };
            let (x, y) = (i % width, i / width);
            let at = [
                i64::from(self.rect.min[0]) + i64::from(x),
                i64::from(self.rect.min[1]) + i64::from(y),
            ];
            let (Ok(px), Ok(py)) = (u32::try_from(at[0]), u32::try_from(at[1])) else {
                continue;
            };
            if px >= self.size[0] || py >= self.size[1] {
                continue;
            }
            let dx = f64::from(x) + 0.5 - centre[0];
            let dy = f64::from(y) + 0.5 - centre[1];
            let d2 = dx * dx + dy * dy;
            let pick = Pick {
                body,
                hit,
                pixel: [px, py],
                point: None,
            };
            if nearest.is_none_or(|(best, ..)| d2 < best) {
                nearest = Some((d2, i, pick));
            }
            if matches!(hit, Element::Edge(_)) && edge.is_none_or(|(best, ..)| d2 < best) {
                edge = Some((d2, i, pick));
            }
        }
        [nearest, edge].map(|found| found.map(|(_, i, pick)| (i, pick)))
    }

    /// The point seen at the centre of the frame's pixel `pixel`, at the picking layer's
    /// `depth` there, in the document: the pixel's centre and the depth unprojected through
    /// the frame's own projection (the picking rectangle's crop moves and scales x and y
    /// alone, so its depth is the frame's), in f64; for an `edge`'s, taken back from the eye
    /// by the reach the strip it was picked from was drawn toward it by ([`back_from_eye`], on
    /// the same pixel): its picking strip's in a frame that picks edges ([`edge_picks`]), its
    /// drawn strip's in one that doesn't ([`drawn_edge_fraction`]); then taken back through
    /// the view (the inverse of [`View::about_camera`], d = right x + up y + back (z +
    /// depth)). None if it isn't finite. (Found by /break-it: taken back by an edge's reach in
    /// both, a frame not picking edges left its point behind the line by what the drawn
    /// strip's reach falls short of it: 0.09 pixels of depth at a pixel a point, 0.44 at 3/4.)
    fn point_at(&self, pixel: [u32; 2], depth: f32, edge: bool) -> Option<[f64; 3]> {
        let centre = pixel.map(|c| f64::from(c) + 0.5);
        let mut p = unproject(self.projection, self.size, centre, f64::from(depth))?;
        if edge {
            p = back_from_eye(p, self.strip_fraction, self.perspective, self.span);
        }
        let v = [p[0], p[1], p[2] + self.depth];
        let f = &self.frame;
        Some(std::array::from_fn(|k| {
            self.origin[k] + f.right[k] * v[0] + f.up[k] * v[1] + f.back[k] * v[2]
        }))
    }
}

/// The view as the fixed camera draws it: the camera's frame in f64, the point the bodies
/// are moved about, and the projection.
struct View {
    frame: CameraFrame,
    /// The point the camera is at, in the document: the eye for a perspective view; for an
    /// orthographic one, the target, the camera then `depth` behind it.
    origin: [f64; 3],
    depth: f64,
    projection: ViewProjection,
    /// How tall the image is, in mm: across the whole view (orthographic), or for each mm
    /// from the eye (perspective, `2 tan(fov / 2)`).
    span: f64,
    perspective: bool,
    /// The image's size in pixels.
    width: u32,
    height: u32,
    /// How much more a position's error moves the image at its corner than at its centre, in
    /// perspective: the corner ray's sec², 1 + tan²(fov/2)(1 + aspect²); 1 orthographic.
    corner: f64,
    /// A perspective view's pyramid, P = { depth ≥ [`MIN_EYE_DISTANCE`], |x| + |y| ≤ B ×
    /// depth } in the eye's frame, by its B, `2^20 t / height` (docs/design/m4.6-render.md,
    /// *A given camera*): what f32 draws to an eighth of a pixel there (the diamond inside
    /// the cone a point projects 2^19 pixels off the axis within, so nothing the clipper's
    /// f32 cuts). Every face and edge is drawn as it meets it, cut in f64; none for an
    /// orthographic view, which draws everything.
    pyramid: Option<f64>,
    /// The image's frustum, about the camera's point: what can have a pixel. A body or face
    /// whose box is wholly outside it isn't placed (`sees`, in the core, culls by it too).
    frustum: ViewRegion,
    /// The pyramid as a region, about the eye; none for an orthographic view.
    inside: Option<ViewRegion>,
    /// The camera it's of, for the frustum an edge's stroke reaches the image from
    /// ([`View::stroke_frustum`]).
    camera: Camera,
}

impl View {
    /// `scene`'s view, from its camera and its bodies, as its check takes them.
    fn of(camera: &Camera, scene: &RenderScene) -> Result<View, String> {
        let frame = camera
            .check(scene.width, scene.height)
            .map_err(|e| e.message)?;
        let bodies = scene.bodies.iter().map(|b| InViewBody::of(b.body, &b.mesh));
        let mut exact = |_: u32, mesh: &Mesh| reach_of_points(mesh, camera.target);
        let in_view = check_bodies_in_view(camera, scene.height, bodies, &mut exact)
            .map_err(|e| e.message)?;
        Ok(View::new(
            camera,
            frame,
            [scene.width, scene.height],
            in_view,
        ))
    }

    /// The view from `camera`, its `frame`, of an image `width` by `height`, of bodies as
    /// their check found them in it ([`check_bodies_in_view`]).
    fn new(
        camera: &Camera,
        frame: CameraFrame,
        [width, height]: [u32; 2],
        in_view: InView,
    ) -> View {
        let frustum = ViewRegion::frustum(camera, &frame, width, height);
        match in_view {
            InView::Orthographic { view_height, reach } => {
                // Only the eye's direction counts: the camera is set past every point of the
                // scene, so nothing is cut off behind the eye (FreeCAD fits its clipping
                // planes to the scene, too), and the far plane past the scene's other side.
                let depth = orthographic_depth(reach);
                View {
                    frame,
                    origin: camera.target,
                    depth,
                    span: view_height,
                    perspective: false,
                    width,
                    height,
                    corner: 1.0,
                    pyramid: None,
                    frustum,
                    inside: None,
                    camera: camera.clone(),
                    projection: view_projection(
                        Projection::Orthographic { view_height },
                        [width, height],
                        (2.0 * depth) as f32,
                    ),
                }
            }
            InView::Perspective { field_of_view } => {
                // What's drawn is cut to the pyramid ([`View::pyramid`]), so nothing drawn is
                // nearer the eye than a nanometre, and the near plane is half that: no
                // triangle crosses it, and the clipper never cuts at the eye. Reverse-z f32
                // depth (re_renderer's infinite reverse projection into a Depth32Float)
                // resolves a depth to 2^-24 of it wherever the near plane is.
                let tan = libm::tan(field_of_view.to_radians() / 2.0);
                View {
                    frame,
                    origin: camera.eye,
                    depth: 0.0,
                    span: 2.0 * tan,
                    perspective: true,
                    width,
                    height,
                    corner: corner_sec2(tan, width, height),
                    pyramid: Some(pyramid_slope(tan, height)),
                    frustum,
                    camera: camera.clone(),
                    inside: Some(ViewRegion::pyramid(
                        camera.eye,
                        &frame,
                        pyramid_slope(tan, height),
                    )),
                    projection: view_projection(
                        Projection::Perspective { field_of_view },
                        [width, height],
                        0.0,
                    ),
                }
            }
        }
    }

    fn world_from_mesh(&self, anchor: [f64; 3]) -> glam::Affine3A {
        world_from_mesh(&self.frame, self.origin, self.depth, anchor)
    }

    /// `p` in the eye's frame, in f64: across the view (right, up) and its depth along it.
    fn eye_frame(&self, p: [f64; 3]) -> [f64; 3] {
        let d = sub(p, self.origin);
        [
            dot(self.frame.right, d),
            dot(self.frame.up, d),
            -dot(self.frame.back, d),
        ]
    }

    /// The frustum a line drawn `width` points wide, at `pixels_per_point`, reaches the image
    /// from: the image's, each side out by half the stroke in pixels.
    fn stroke_frustum(&self, width: f32, pixels_per_point: f32) -> ViewRegion {
        let half = f64::from(width) / 2.0 * f64::from(pixels_per_point);
        ViewRegion::frustum_widened(&self.camera, &self.frame, self.width, self.height, half)
    }

    /// Where a face (or an edge) with the box `bounds` and the corners `vertices` is drawn in
    /// this view: not at all where its box is wholly outside `frustum` (the image's, or for an
    /// edge, the one its stroke reaches it from) or the pyramid, or its
    /// corners are wholly outside the pyramid (no clipping: the eight corners, then its
    /// vertices); as it is where its box or its corners are wholly in the pyramid; and cut to
    /// it only where its own corners cross it.
    fn place<'a>(
        &self,
        frustum: &ViewRegion,
        bounds: Bounds,
        vertices: impl Fn() -> Box<dyn Iterator<Item = [f64; 3]> + 'a>,
    ) -> Placed {
        if frustum.class_of_box(bounds) == Class::Out {
            return Placed::Not;
        }
        let Some(inside) = &self.inside else {
            return Placed::Whole;
        };
        let class = match inside.class_of_box(bounds) {
            Class::Crosses => inside.class(vertices()),
            class => class,
        };
        match class {
            Class::In => Placed::Whole,
            Class::Out => Placed::Not,
            Class::Crosses => Placed::Cut,
        }
    }

    /// The pyramid's `k`th half-space (of five) as a linear form of a point in the eye's
    /// frame, at least 0 inside; none of a view with no pyramid.
    fn side(&self, k: usize, p: [f64; 3]) -> f64 {
        self.inside
            .as_ref()
            .map_or(0.0, |inside| inside.sides[k].value(p))
    }

    /// What identifies the pyramid in the world: the eye, the view's axes, and B, by their
    /// bits; a face cut to it is cut again only when it moves.
    fn cut_key(&self) -> [u64; 13] {
        let f = &self.frame;
        let mut key = [0u64; 13];
        for (k, v) in self
            .origin
            .iter()
            .chain(&f.right)
            .chain(&f.up)
            .chain(&f.back)
            .chain(self.pyramid.iter())
            .enumerate()
        {
            key[k] = v.to_bits();
        }
        key
    }

    /// `d`, a point in the eye's frame, as an offset from the eye in the document's axes.
    fn offset_of(&self, [x, y, depth]: [f64; 3]) -> [f64; 3] {
        let f = &self.frame;
        std::array::from_fn(|k| f.right[k] * x + f.up[k] * y - f.back[k] * depth)
    }

    /// What of the segment from `a` to `b` (in the eye's frame) lies in the pyramid: the
    /// fractions along it it enters and leaves at, or none, each side cut in turn by the one
    /// rule ([`cut_by`]). Of a view with no pyramid, all.
    fn cut_segment(&self, a: [f64; 3], b: [f64; 3]) -> Option<(f64, f64)> {
        let Some(inside) = &self.inside else {
            return Some((0.0, 1.0));
        };
        inside.sides.iter().try_fold((0.0, 1.0), |kept, side| {
            cut_by(side.value(a), side.value(b), kept)
        })
    }

    /// The point a face with the box `bounds` is drawn about in this view: its box's centre
    /// where f32 about it is precise to [`PRECISION_OF_PIXEL`] at the image's corner, so the
    /// next view of it uploads nothing; otherwise (a large face seen close) the camera's
    /// point, the eye or the target, as M4.6 drew everything, its mesh uploaded for this
    /// view. A position about its box's centre rounds by at most the box's half diagonal
    /// times [`F32_ROUNDING`], against the pixel at the box's point nearest the eye in
    /// perspective (none, for an eye in the box). About the eye, a position's rounding grows
    /// with its distance from it, as the pixel does, so a perspective view takes the eye
    /// then; an orthographic one takes the point on its line through the target at the
    /// face's depth, where what it sees of the face is.
    fn anchor(&self, bounds: Bounds) -> [f64; 3] {
        let (low, high) = bounds;
        let half: [f64; 3] = std::array::from_fn(|k| high[k] / 2.0 - low[k] / 2.0);
        let half = dot(half, half).sqrt();
        let nearest: [f64; 3] =
            std::array::from_fn(|k| self.origin[k].clamp(low[k], high[k]) - self.origin[k]);
        let nearest = dot(nearest, nearest).sqrt();
        let across = if self.perspective {
            nearest * self.span
        } else {
            self.span
        };
        let precise = half * F32_ROUNDING * self.corner
            <= across / f64::from(self.height) * PRECISION_OF_PIXEL;
        let centre = centre_of(bounds);
        if precise {
            centre
        } else if self.perspective {
            self.origin
        } else {
            // On the view's line through the target, at the face's depth: about it, a
            // position's rounding grows with its distance from what the view sees, and not
            // with how far along the ray the target is.
            let along = dot(self.frame.back, sub(centre, self.origin));
            std::array::from_fn(|k| self.origin[k] + self.frame.back[k] * along)
        }
    }

    /// The line strip through `points` cut to the view's pyramid: each segment clipped in
    /// f64 in the eye's frame, a cut ending the strip and the next kept segment beginning
    /// another; one strip, of every point, in a view with no pyramid. Each point it keeps or
    /// cuts at, about the camera in f64 ([`View::about_camera`]'s), is placed by `place`:
    /// moved toward the eye ([`View::strip_from_camera`]), a drawn edge's by its own reach
    /// ([`edges`]) and an edge's picking strip by an edge's ([`edge_picks`]), so each is cut
    /// where the edge's line is, then moved.
    fn cut_strip(
        &self,
        points: &[[f64; 3]],
        place: impl Fn([f64; 3]) -> glam::Vec3,
    ) -> Vec<Vec<glam::Vec3>> {
        if self.pyramid.is_none() {
            return vec![
                points
                    .iter()
                    .map(|p| place(self.about_camera(*p)))
                    .collect(),
            ];
        }
        // In the eye's frame about the eye (a pyramid's view is a perspective one, its camera
        // the eye), its depth the negated z.
        let placed = |[x, y, depth]: [f64; 3]| place([x, y, -depth]);
        let mut strips = Vec::new();
        let mut strip: Vec<glam::Vec3> = Vec::new();
        for pair in points.windows(2) {
            let (a, b) = (self.eye_frame(pair[0]), self.eye_frame(pair[1]));
            let Some((enter, leave)) = self.cut_segment(a, b) else {
                if strip.len() > 1 {
                    strips.push(std::mem::take(&mut strip));
                }
                strip.clear();
                continue;
            };
            let at = |s: f64| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * s);
            if enter > 0.0 || strip.is_empty() {
                if strip.len() > 1 {
                    strips.push(std::mem::take(&mut strip));
                }
                strip.clear();
                strip.push(placed(at(enter)));
            }
            strip.push(placed(at(leave)));
            if leave < 1.0 {
                strips.push(std::mem::take(&mut strip));
            }
        }
        if strip.len() > 1 {
            strips.push(strip);
        }
        strips
    }

    /// Where a vertex at `p`, a point in the document, has its picking disc: about the camera
    /// ([`View::about_camera`]), moved toward the eye by its reach ([`vertex_reach`], which
    /// Select Other's cast takes too, [`toward_eye`]), in f64 before it's f32; along the view
    /// (orthographic, at most halfway to the near plane, at the camera, z = 0) or along its
    /// line to the eye (perspective, at most [`MOST_REACH`] of its depth), so it lands on the
    /// same pixel.
    fn disc_at(&self, p: [f64; 3]) -> glam::Vec3 {
        let v = self.about_camera(p);
        vec3(toward_eye(v, VERTEX_REACH, self.perspective, self.span))
    }

    /// Where an edge's strip has `about_camera`, a point about the camera already
    /// ([`View::about_camera`]'s, not the document's, as [`View::disc_at`] takes: a strip's
    /// points are cut there first, [`View::cut_strip`]): moved toward the eye by `fraction` of
    /// the view's height ([`toward_eye`]), as a vertex's disc is by its own reach, in f64
    /// before it's f32. A drawn strip's is [`drawn_edge_fraction`]; a picking strip's an
    /// edge's reach, [`edge_fraction`], of the view's rows, which Select Other's cast takes too
    /// ([`edge_reach`]).
    fn strip_from_camera(&self, about_camera: [f64; 3], fraction: f64) -> glam::Vec3 {
        vec3(toward_eye(
            about_camera,
            fraction,
            self.perspective,
            self.span,
        ))
    }

    /// `p` in the fixed camera's world, in f64: its offset from the camera's point rotated
    /// into the view, x right, y up, and z toward the eye, the camera at z = 0. Both taken in
    /// f64 before it's f32 ([`vec3`]), it's precise to f32's step of what's across the view,
    /// wherever the point is.
    fn about_camera(&self, p: [f64; 3]) -> [f64; 3] {
        let d = sub(p, self.origin);
        [
            dot(self.frame.right, d),
            dot(self.frame.up, d),
            dot(self.frame.back, d) - self.depth,
        ]
    }
}

/// The projection a view of `projection` at `size_px` is drawn with, about the fixed camera
/// (at the origin, looking along −z with +y up): orthographic about its near plane's centre,
/// `far` behind it (the scene's depth, which a view's x and y don't depend on); or in
/// perspective from the eye, its near plane half [`MIN_EYE_DISTANCE`] away and nothing far.
/// One for drawing ([`View::new`]), picking's points ([`PendingPick::point_at`]), and
/// [`ray`], so they can't disagree.
fn view_projection(projection: Projection, [width, height]: [u32; 2], far: f32) -> ViewProjection {
    match projection {
        Projection::Orthographic { view_height } => ViewProjection::Orthographic {
            camera_mode: OrthographicCameraMode::NearPlaneCenter,
            vertical_world_size: view_height as f32,
            far_plane_distance: far,
        },
        Projection::Perspective { field_of_view } => ViewProjection::Perspective {
            vertical_fov: field_of_view.to_radians() as f32,
            near_plane_distance: (MIN_EYE_DISTANCE / 2.0) as f32,
            aspect_ratio: width as f32 / height as f32,
        },
    }
}

/// The point at `pixel` (from the image's top-left, x right and y down, in pixels: a pixel's
/// centre is half a pixel in) and the normalized device depth `depth`, `size` in pixels,
/// through `projection`'s matrix as the GPU takes it (its f32 entries), inverted in f64: in
/// the fixed camera's frame; none if it isn't finite.
fn unproject(
    projection: ViewProjection,
    size: [u32; 2],
    pixel: [f64; 2],
    depth: f64,
) -> Option<[f64; 3]> {
    let [width, height] = size.map(f64::from);
    let ndc = glam::DVec3::new(
        pixel[0] / width * 2.0 - 1.0,
        1.0 - pixel[1] / height * 2.0,
        depth,
    );
    let view_from_projection = projection.projection_from_view(size).as_dmat4().inverse();
    let p = view_from_projection.project_point3(ndc);
    p.is_finite().then_some([p.x, p.y, p.z])
}

/// The ray through `pixel` of a view from `camera` `size_px` in pixels, in the document's
/// coordinates, in f64: its origin, and its direction of unit length, into the view. `pixel`
/// is from the image's top-left, x right and y down, in pixels (a pixel's centre is half a
/// pixel in, so the image's centre is `size_px / 2`); one outside the image is a ray all the
/// same. By the projection the renderer draws with ([`view_projection`]), so it passes through
/// the point a frame's picking finds at that pixel ([`Pick::point`]):
/// - in perspective, from the eye;
/// - orthographic, parallel to the view, from the view's plane (through the target, square to
///   the view). An orthographic view draws everything along its line, in front of that plane
///   too (its camera is set past the scene), so a pick meets the whole line, not only what's
///   ahead of its origin.
///
/// None for a camera [`Camera::check`] refuses at this size, a size of 0 on a side, a pixel
/// that isn't finite, or one so far off the image that the ray's origin or direction isn't.
pub fn ray(camera: &Camera, size_px: [u32; 2], pixel: [f64; 2]) -> Option<([f64; 3], [f64; 3])> {
    let [width, height] = size_px;
    if width == 0 || height == 0 || !pixel.iter().all(|c| c.is_finite()) {
        return None;
    }
    let frame = camera.check(width, height).ok()?;
    let into_document = |[x, y, z]: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|k| frame.right[k] * x + frame.up[k] * y + frame.back[k] * z)
    };
    match camera.projection {
        Projection::Perspective { .. } => {
            // The point on the near plane, in the eye's frame: the ray's way from the eye.
            let projection = view_projection(camera.projection, size_px, 0.0);
            let along = into_document(unproject(projection, size_px, pixel, 1.0)?);
            // Never 0: the near plane's point is `near` along the view (its z, the inverse's
            // of device depth 1), and the rotation keeps its length.
            let length = dot(along, along).sqrt();
            length
                .is_finite()
                .then(|| (camera.eye, along.map(|c| c / length)))
        }
        Projection::Orthographic { .. } => {
            // Across the view only: x and y don't depend on the depth, or on the far plane.
            let projection = view_projection(camera.projection, size_px, 1.0);
            let [x, y, _] = unproject(projection, size_px, pixel, 0.5)?;
            let across = into_document([x, y, 0.0]);
            let origin: [f64; 3] = std::array::from_fn(|k| camera.target[k] + across[k]);
            // A pixel far enough off the image puts the origin past f64: none.
            origin
                .iter()
                .all(|c| c.is_finite())
                .then(|| (origin, frame.back.map(|c| -c)))
        }
    }
}

/// Where `world`, a point in the document's coordinates, lands in a view from `camera`
/// `size_px` in pixels: from the image's top-left, x right and y down, in pixels (a pixel's
/// centre is half a pixel in), in f64. The inverse of [`ray`]: by the projection the renderer
/// draws with ([`view_projection`], its f32 matrix taken in f64), so it lands where the
/// bodies are drawn, and a point on a pixel's ray lands on that pixel. A point off the image
/// lands off it, all the same.
///
/// None, in perspective, for a point nearer the eye along the view than
/// [`MIN_EYE_DISTANCE`] (behind it among them), where the drawing cuts what it draws (its
/// pyramid, depth ≥ [`MIN_EYE_DISTANCE`]), so what's drawn over the view and the bodies
/// agree on what's seen. An orthographic view has no behind: any finite point projects.
/// None, too, for a camera [`Camera::check`] refuses at this size, a size of 0 on a side, a
/// point that isn't finite, or one whose pixel isn't.
pub fn project(camera: &Camera, size_px: [u32; 2], world: [f64; 3]) -> Option<[f64; 2]> {
    Projector::new(camera, size_px)?.pixel(world)
}

/// [`project`]'s projection of one view, made once for many points: the camera's frame and
/// point, and the matrix, in f64.
struct Projector {
    frame: CameraFrame,
    origin: [f64; 3],
    perspective: bool,
    matrix: glam::DMat4,
    size: [f64; 2],
}

impl Projector {
    /// The projection of a view from `camera` `size_px` in pixels; none for a camera
    /// [`Camera::check`] refuses at this size, or a size of 0 on a side.
    fn new(camera: &Camera, size_px: [u32; 2]) -> Option<Projector> {
        let [width, height] = size_px;
        if width == 0 || height == 0 {
            return None;
        }
        let frame = camera.check(width, height).ok()?;
        // About the camera's point, as [`View`] draws: the eye in perspective, the target
        // orthographic (whose depth x and y don't depend on).
        let (origin, far, perspective) = match camera.projection {
            Projection::Perspective { .. } => (camera.eye, 0.0, true),
            Projection::Orthographic { .. } => (camera.target, 1.0, false),
        };
        let projection = view_projection(camera.projection, size_px, far);
        Some(Projector {
            frame,
            origin,
            perspective,
            matrix: projection.projection_from_view(size_px).as_dmat4(),
            size: [f64::from(width), f64::from(height)],
        })
    }

    /// Where `world` lands, as [`project`] says.
    fn pixel(&self, world: [f64; 3]) -> Option<[f64; 2]> {
        if !world.iter().all(|c| c.is_finite()) {
            return None;
        }
        let view = self.view_of(world);
        // Its depth as the drawing's pyramid takes it ([`View::eye_frame`]); NaN where the
        // point is so far that its offsets overflow.
        let depth = -view.z;
        if self.perspective && (depth.is_nan() || depth < MIN_EYE_DISTANCE) {
            return None;
        }
        self.landing(view).map(|(pixel, _)| pixel)
    }

    /// `world` in the fixed camera's frame, about the camera's point: across the view (x
    /// right, y up) and along it (z, toward the eye), in f64.
    fn view_of(&self, world: [f64; 3]) -> glam::DVec4 {
        let frame = &self.frame;
        let d = sub(world, self.origin);
        glam::DVec4::new(
            dot(frame.right, d),
            dot(frame.up, d),
            dot(frame.back, d),
            1.0,
        )
    }

    /// What of the segment from `va` to `vb` (points about the eye, [`view_of`](Self::view_of))
    /// a perspective drawing keeps: what's at least [`MIN_EYE_DISTANCE`] from the eye along
    /// the view, its pyramid's near side ([`ViewRegion::pyramid`]'s first, its form `depth −
    /// MIN_EYE_DISTANCE`), cut by the drawing's own rule ([`cut_by`]); none where nothing of
    /// it is, or an end's depth isn't finite.
    fn near_cut(va: glam::DVec4, vb: glam::DVec4) -> Option<(f64, f64)> {
        let (da, db) = (-va.z, -vb.z);
        if !(da.is_finite() && db.is_finite()) {
            return None;
        }
        cut_by(da - MIN_EYE_DISTANCE, db - MIN_EYE_DISTANCE, (0.0, 1.0))
    }

    /// Where `view`, a point in the fixed camera's frame ([`view_of`](Self::view_of)), lands
    /// through the matrix, cut or not, and its clip w (its depth in perspective, 1
    /// orthographic); none if either isn't finite.
    fn landing(&self, view: glam::DVec4) -> Option<([f64; 2], f64)> {
        let clip = self.matrix * view;
        let pixel = [
            (clip.x / clip.w + 1.0) / 2.0 * self.size[0],
            (1.0 - clip.y / clip.w) / 2.0 * self.size[1],
        ];
        (pixel.iter().all(|c| c.is_finite()) && clip.w.is_finite()).then_some((pixel, clip.w))
    }
}

/// The most [`hits`] gives, the nearest: no list shows more, and each edge or vertex given
/// costs a cast of its own (whether a face hides it), so a crowd under one pixel (a mesh's
/// thousands of points there) costs no more than this many of those. They're chosen from
/// the rest in time linear in how many there are, before anything more is done with them.
pub const MAX_HITS: usize = 256;

/// How near the pointer, in points, an edge's drawn line is taken: the approved prototype's
/// (docs/design/m5-ui, viewport.js, `pickAll`: `best.d < 6`), which its `pick`, the hover's
/// and the click's, takes from `pickAll` too. [`hits`] takes an edge whose polyline passes
/// nearer the pointer than this; the app's hover and click read a picking square this far
/// out on each side of the pointer's pixel.
pub const HIT_EDGE_POINTS: f64 = 6.0;

/// How near the pointer, in points, a vertex is taken: the prototype's `d < 7` (its `pickAll`,
/// and so its `pick`), the radius of the disc a frame picks a vertex within.
pub const HIT_VERTEX_POINTS: f64 = 7.0;

/// The most triangles of one face a [`Pickable`]'s chunk holds, run together in the mesh's
/// order.
const CHUNK_TRIANGLES: usize = 8;

/// The most chunks a leaf of a [`Pickable`]'s tree holds.
const LEAF_CHUNKS: usize = 4;

#[cfg(test)]
thread_local! {
    /// How many triangles casts on this thread have tested a line against: for the tests
    /// that bound a cast's work.
    static TRIANGLES_TESTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// How many of a [`Tree`]'s boxes (its nodes' and its chunks') casts on this thread have
    /// tested a line against: for the tests that bound a cast's work.
    static BOXES_TESTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// How many edges frames on this thread have found near their picking squares, each drawn
    /// again for picking where its view places it ([`edge_picks`]): for the tests that bound
    /// a frame's picking work.
    static EDGE_PICKS_DRAWN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A body's mesh as [`hits`] casts on it: the mesh (the same `Arc` the renderer was given);
/// its box, of its positions, edge points, and vertex points, and the one the renderer's
/// view takes it by, of its positions and edge points; its triangles in a tree of boxes;
/// and each edge's box: made once ([`Pickable::new`]), so a cast tests a box before what's
/// in it.
#[derive(Debug, Clone)]
pub struct Pickable {
    mesh: Arc<Mesh>,
    bounds: Option<Bounds>,
    drawn: Option<Bounds>,
    triangles: Tree,
    edges: Vec<Option<Bounds>>,
}

impl Pickable {
    /// `mesh`'s boxes, for casts on it: any mesh, checked or not (a triangle indexing past
    /// its positions, or counts that don't add up, is passed over where it's met, never
    /// read past). In time linear in its size, and the log of its triangles' number.
    pub fn new(mesh: Arc<Mesh>) -> Pickable {
        let _span = tracing::debug_span!("pickable", faces = mesh.face_triangles.len()).entered();
        Pickable {
            bounds: box_of(
                mesh.positions
                    .iter()
                    .chain(&mesh.edge_points)
                    .chain(&mesh.vertex_points),
            ),
            drawn: box_of(mesh.positions.iter().chain(&mesh.edge_points)),
            triangles: Tree::of(&mesh),
            edges: edge_spans(&mesh)
                .map(|points| box_of(points.iter()))
                .collect(),
            mesh,
        }
    }

    /// The mesh it was made from.
    pub fn mesh(&self) -> &Arc<Mesh> {
        &self.mesh
    }
}

/// A body's triangles as a tree of boxes (a bounding volume hierarchy), made once with its
/// [`Pickable`], so a line meets them in about the log of their number: each face's
/// triangles in chunks of at most [`CHUNK_TRIANGLES`] run together in the mesh's order, each
/// with its box; and over the chunks, a tree, each node's box holding its chunks', each leaf
/// at most [`LEAF_CHUNKS`] of them. A node's chunks are split at the median of their boxes'
/// centres along the axis those spread most, ties by their place in the mesh: the same tree
/// for the same mesh, every time.
///
/// What a walk of it costs is the boxes the line comes within rounding of: about twice the
/// log of the chunks' number for a line through a mesh whose boxes don't overlap there, the
/// most for one whose triangles' boxes all hold the line and none of which it meets (slivers
/// crowded about one line, each of some area): then every box and every triangle, for each
/// of a cast's walks ([`MAX_HITS`] and the pointer's). That's accepted: no kernel's mesh is
/// that, and a triangle of no area that no line meets is never in the tree ([`Tree::of`]).
#[derive(Debug, Clone, Default)]
struct Tree {
    nodes: Vec<TreeNode>,
    chunks: Vec<Chunk>,
}

/// Of a [`Tree`]: `count` triangles of the face numbered `face`, from `first` in the mesh's
/// triangles, each of whose corners is one of its positions and that a line can meet
/// ([`can_meet`]), and their corners' box.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    first: usize,
    count: usize,
    face: u32,
    bounds: Bounds,
}

/// A node of a [`Tree`]: its box, and what it holds.
#[derive(Debug, Clone, Copy)]
struct TreeNode {
    bounds: Bounds,
    holds: Holds,
}

impl TreeNode {
    /// A node [`Tree::of`] hasn't filled yet, of box `bounds`: a leaf of no chunks, so one
    /// left unfilled (by a range of chunks that can't be) is walked as nothing, never as a
    /// node whose children are the root again, round for ever.
    fn unfilled(bounds: Bounds) -> TreeNode {
        TreeNode {
            bounds,
            holds: Holds::Chunks { first: 0, count: 0 },
        }
    }
}

/// What a [`TreeNode`] holds: a leaf's chunks, `count` of the tree's from `first`; or an
/// inner node's two children, the nodes at `left` and the one after it.
#[derive(Debug, Clone, Copy)]
enum Holds {
    Chunks { first: usize, count: usize },
    Children { left: usize },
}

/// The box holding the boxes `a` and `b`.
fn joined((low, high): Bounds, (l, h): Bounds) -> Bounds {
    (
        std::array::from_fn(|k| low[k].min(l[k])),
        std::array::from_fn(|k| high[k].max(h[k])),
    )
}

/// The box holding `boxes`; none for none.
fn union(mut boxes: impl Iterator<Item = Bounds>) -> Option<Bounds> {
    let first = boxes.next()?;
    Some(boxes.fold(first, joined))
}

/// Whether some line could meet the triangle `[a, b, c]` ([`Line::meets`]): not if its first
/// corner is repeated (`b` or `c` the same point as `a`), as every one of a mesh's
/// triangles at one point is. Then one of the two sides `meets` crosses is exactly 0, and so
/// is what it divides by (or that isn't finite), whatever the line: such a triangle is never
/// met, and leaving it out changes nothing a cast finds.
fn can_meet([a, b, c]: [[f64; 3]; 3]) -> bool {
    a != b && a != c
}

impl Tree {
    /// `mesh`'s tree: each face's triangles, its triangles that run past the mesh's passed
    /// over whole (as a face whose count doesn't add up is); a triangle a corner of which
    /// isn't a position, or that no line can meet ([`can_meet`]), left out, ending the run a
    /// chunk holds. So a mesh of a hundred thousand triangles at one point has nothing in
    /// its tree, and a cast tests no box of it, not a hundred thousand for each of its walks.
    fn of(mesh: &Mesh) -> Tree {
        let mut chunks = Vec::new();
        let mut first = 0usize;
        for (face, &count) in (1u32..).zip(&mesh.face_triangles) {
            let count = usize::try_from(count).unwrap_or(usize::MAX);
            let end = first.saturating_add(count);
            if let Some(triangles) = mesh.triangles.get(first..end) {
                // The chunk being filled: the triangles from its first on, and their box.
                let mut run: Option<Chunk> = None;
                for (at, triangle) in (first..).zip(triangles) {
                    let met = corners_of(mesh, triangle)
                        .filter(|corners| can_meet(*corners))
                        .and_then(|corners| box_of(corners.iter()));
                    let Some(bounds) = met else {
                        chunks.extend(run.take());
                        continue;
                    };
                    let chunk = run.get_or_insert(Chunk {
                        first: at,
                        count: 0,
                        face,
                        bounds,
                    });
                    chunk.count += 1;
                    chunk.bounds = joined(chunk.bounds, bounds);
                    if chunk.count == CHUNK_TRIANGLES {
                        chunks.extend(run.take());
                    }
                }
                chunks.extend(run);
            }
            first = end;
        }
        let mut nodes = Vec::new();
        let Some(bounds) = union(chunks.iter().map(|chunk| chunk.bounds)) else {
            return Tree::default();
        };
        let placeholder = TreeNode::unfilled(bounds);
        nodes.push(placeholder);
        // Each node to fill, and its chunks' range: a stack, as deep as the tree is (about the
        // log of the chunks' number, each split halving them).
        let mut stack = vec![(0usize, 0usize, chunks.len())];
        while let Some((node, start, end)) = stack.pop() {
            let Some(held) = chunks.get_mut(start..end) else {
                continue;
            };
            let Some(bounds) = union(held.iter().map(|chunk| chunk.bounds)) else {
                continue;
            };
            let holds = if held.len() <= LEAF_CHUNKS {
                Holds::Chunks {
                    first: start,
                    count: held.len(),
                }
            } else {
                let centres = union(held.iter().map(|chunk| {
                    let centre = centre_of(chunk.bounds);
                    (centre, centre)
                }));
                let axis = centres.map_or(0, |(low, high)| {
                    (1..3).fold(0, |best, k| {
                        if high[k] - low[k] > high[best] - low[best] {
                            k
                        } else {
                            best
                        }
                    })
                });
                let half = held.len() / 2;
                held.select_nth_unstable_by(half, |a, b| {
                    centre_of(a.bounds)[axis]
                        .total_cmp(&centre_of(b.bounds)[axis])
                        .then(a.first.cmp(&b.first))
                });
                let left = nodes.len();
                nodes.push(placeholder);
                nodes.push(placeholder);
                stack.push((left + 1, start + half, end));
                stack.push((left, start, start + half));
                Holds::Children { left }
            };
            if let Some(slot) = nodes.get_mut(node) {
                *slot = TreeNode { bounds, holds };
            }
        }
        Tree { nodes, chunks }
    }

    /// Each triangle of `mesh` (whose tree it is) `line` meets, where `caster` draws it (in
    /// perspective, no nearer the eye than [`MIN_EYE_DISTANCE`]): its face's number, the
    /// point, and its depth, given to `met` until it says to stop; of the boxes (the nodes',
    /// then the chunks') `line` comes within rounding of, and `wanted` keeps, alone. Whether
    /// `met` stopped it.
    fn walk(
        &self,
        caster: &Caster,
        mesh: &Mesh,
        line: &Line,
        wanted: impl Fn(Bounds) -> bool,
        mut met: impl FnMut(u32, [f64; 3], f64) -> bool,
    ) -> bool {
        let near = |bounds: Bounds| {
            #[cfg(test)]
            BOXES_TESTED.with(|tested| tested.set(tested.get() + 1));
            line.meets_box(bounds, slack_of(bounds)) && wanted(bounds)
        };
        let mut stack = Vec::new();
        if !self.nodes.is_empty() {
            stack.push(0usize);
        }
        while let Some(at) = stack.pop() {
            let Some(node) = self.nodes.get(at) else {
                continue;
            };
            if !near(node.bounds) {
                continue;
            }
            let (first, count) = match node.holds {
                Holds::Children { left } => {
                    stack.push(left.saturating_add(1));
                    stack.push(left);
                    continue;
                }
                Holds::Chunks { first, count } => (first, count),
            };
            let chunks = self
                .chunks
                .get(first..first.saturating_add(count))
                .unwrap_or(&[]);
            for chunk in chunks.iter().filter(|chunk| near(chunk.bounds)) {
                let run = mesh
                    .triangles
                    .get(chunk.first..chunk.first.saturating_add(chunk.count))
                    .unwrap_or(&[]);
                for triangle in run {
                    let Some(corners) = corners_of(mesh, triangle) else {
                        continue;
                    };
                    #[cfg(test)]
                    TRIANGLES_TESTED.with(|tested| tested.set(tested.get() + 1));
                    let Some(t) = line.meets(corners) else {
                        continue;
                    };
                    let point: [f64; 3] =
                        std::array::from_fn(|k| line.origin[k] + line.direction[k] * t);
                    let depth = caster.depth_on(line, t, point);
                    if caster.drawn(depth) && met(chunk.face, point, depth) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// What's under a pointer ([`hits`]): a body's face, edge, or vertex; where it's met (a
/// face's point on the pointer's ray, an edge's point nearest the pointer in the image, a
/// vertex's own point), in the document's coordinates; its depth there, along the view from
/// the eye, in mm (in front of an orthographic eye, below 0, as it's drawn); and whether a
/// face hides it there (the approved prototype's "· hidden").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub body: u32,
    pub element: Element,
    pub point: [f64; 3],
    pub depth: f64,
    pub hidden: bool,
}

/// Everything under `pointer` in a frame from `camera`, `size_px` physical pixels at
/// `pixels_per_point`, of `bodies` (each body's number and its [`Pickable`]: the bodies the
/// frame drew, their meshes the renderer's), nearest the eye first (Select Other, M5.7;
/// docs/design/m5.0-app.md, section 9). `pointer` is in physical pixels from the image's top
/// left, as [`ray`] takes it. Pure f64, on the CPU, by the projection the renderer draws with
/// ([`ray`], [`project`]), every kind of element whatever the selection's filters:
/// - each face whose triangles the pointer's ray meets (each body's tree of boxes tested
///   first), at its nearest meeting;
/// - each edge whose drawn polyline passes within [`HIT_EDGE_POINTS`] of the pointer in the
///   image, at its point nearest it there (of a segment seen end on, its end nearest the
///   eye); and each vertex within [`HIT_VERTEX_POINTS`], at its own point.
///
/// As the drawing cuts: in perspective nothing nearer the eye than [`MIN_EYE_DISTANCE`]
/// along the view is met (each edge cut there about the eye before it's projected, by the
/// drawing's own rule, [`cut_by`], as [`project_segment`] cuts and `project` a point), and
/// orthographic the whole line is (drawn in front of the eye too).
///
/// A vertex is taken toward the eye by the reach a frame's picking disc is drawn toward it by
/// ([`vertex_reach`]: [`VERTEX_REACH`] of the view's height at it, the prototype's `pickAll`
/// tolerance, `eps`; orthographic, at most halfway to the near plane the renderer draws the
/// bodies from, as it sets it for them; in perspective, at most [`MOST_REACH`] of its
/// depth), and an edge by the reach its picking strip is drawn toward it by ([`edge_reach`]:
/// [`EDGE_REACH_PIXELS`] of the view's depth at it, with the same caps, never more than a
/// vertex's), so the hover, the click, and the cast take an edge alike: each is listed by
/// its depth less its reach, and hidden only where a face is met nearer than that on the ray
/// through its own point. So one lying on the faces it bounds is never hidden by them, at any
/// angle, and comes before the face under the pointer within that reach, as the renderer's
/// picking draws it over its face. A face is hidden where another face is met strictly
/// nearer on the pointer's ray.
/// Ordered by that depth, then vertex, edge, and face, then body and number, with no
/// tolerance more but the triangles' own: a line along an edge two triangles share meets
/// both ([`WATERTIGHT`], a part in 2^40 of a triangle).
///
/// The nearest [`MAX_HITS`] of them, if there are more: chosen first, in time linear in how
/// many there are, and only they are cast on again, to say whether a face hides each. None
/// for a camera [`Camera::check`] refuses at this size, a size of 0 on a side, a
/// `pixels_per_point` that isn't above 0 and at most [`MAX_PIXELS_PER_POINT`], bodies the
/// renderer's view refuses (an orthographic view too small for them,
/// [`check_bodies_in_view`]: it draws nothing of them), or a pointer that isn't finite and
/// inside the image (where nothing is drawn).
pub fn hits(
    camera: &Camera,
    size_px: [u32; 2],
    pixels_per_point: f32,
    bodies: &[(u32, &Pickable)],
    pointer: [f64; 2],
) -> Vec<Hit> {
    hits_among(camera, size_px, pixels_per_point, bodies, &[], pointer)
}

/// [`hits`] in a frame that draws `lines` too ([`Frame::lines`]'s strips): they're never hit,
/// but an orthographic frame's reach takes them, which sets its near plane and so how far an
/// edge or a vertex is taken toward the eye ([`vertex_reach`]'s cap), so the cast takes them
/// as the frame does. None, too, for lines the frame's view refuses (`lines`).
pub fn hits_among(
    camera: &Camera,
    size_px: [u32; 2],
    pixels_per_point: f32,
    bodies: &[(u32, &Pickable)],
    lines: &[&Strips],
    pointer: [f64; 2],
) -> Vec<Hit> {
    let _span = tracing::debug_span!("hits", bodies = bodies.len(), lines = lines.len()).entered();
    let Some((caster, line)) =
        cast_through(camera, size_px, pixels_per_point, bodies, lines, pointer)
    else {
        return Vec::new();
    };
    let mut found: Vec<Found> = Vec::new();
    for &(body, pickable) in bodies {
        caster.faces_met(body, pickable, &line, &mut found);
        caster.edges_near(body, pickable, &line, pointer, &mut found);
        caster.vertices_near(body, pickable, &line, pointer, &mut found);
    }
    let nearest_face = found
        .iter()
        .filter(|f| matches!(f.hit.element, Element::Face(_)))
        .map(|f| f.hit.depth)
        .fold(f64::INFINITY, f64::min);
    let order = |a: &Found, b: &Found| {
        a.key
            .total_cmp(&b.key)
            .then_with(|| rank(a.hit.element).cmp(&rank(b.hit.element)))
            .then_with(|| a.hit.body.cmp(&b.hit.body))
            .then_with(|| a.hit.element.cmp(&b.hit.element))
    };
    let under = found.len();
    if found.len() > MAX_HITS {
        // The nearest MAX_HITS first, in time linear in how many are under the pointer: the
        // rest are never sorted, nor cast on again.
        found.select_nth_unstable_by(MAX_HITS, order);
        found.truncate(MAX_HITS);
    }
    found.sort_by(order);
    for each in &mut found {
        each.hit.hidden = match each.hit.element {
            Element::Face(_) => nearest_face < each.hit.depth,
            Element::Edge(_) | Element::Vertex(_) => caster.hidden(bodies, each),
        };
    }
    tracing::debug!(hits = found.len(), under, "cast");
    found.into_iter().map(|f| f.hit).collect()
}

/// The face under `pointer` nearest the eye: of the faces [`hits`] lists for the same frame,
/// bodies, and pointer, the first (by depth, then body, then number), refused as `hits`
/// refuses them (none). It's the face a frame's picking holds at that pixel where nothing is
/// drawn over it: the selection's where an edge's stroke covers the pointer and the filter
/// takes faces but not edges, as SolidWorks' face filter selects only faces
/// (docs/ux/solidworks.md §1, *Selection filters*). Only faces are cast on: no edge or vertex is
/// sought, so none can crowd it out of `hits`' [`MAX_HITS`]. Its `hidden` is false: no face
/// is met nearer.
pub fn face_hit(
    camera: &Camera,
    size_px: [u32; 2],
    pixels_per_point: f32,
    bodies: &[(u32, &Pickable)],
    pointer: [f64; 2],
) -> Option<Hit> {
    let _span = tracing::debug_span!("face_hit", bodies = bodies.len()).entered();
    let (caster, line) = cast_through(camera, size_px, pixels_per_point, bodies, &[], pointer)?;
    let mut found: Vec<Found> = Vec::new();
    for &(body, pickable) in bodies {
        caster.faces_met(body, pickable, &line, &mut found);
    }
    found.into_iter().map(|f| f.hit).min_by(|a, b| {
        a.depth
            .total_cmp(&b.depth)
            .then_with(|| a.body.cmp(&b.body))
            .then_with(|| a.element.cmp(&b.element))
    })
}

/// [`hits_among`]' and [`face_hit`]'s view of `bodies` (in a frame drawing `lines` too) and
/// the line through `pointer` they cast along: none for what `hits_among` refuses (a
/// `pixels_per_point` that isn't above 0 and at most [`MAX_PIXELS_PER_POINT`], a pointer that
/// isn't finite and inside the image, a camera, bodies, or lines the view refuses).
fn cast_through(
    camera: &Camera,
    size_px: [u32; 2],
    pixels_per_point: f32,
    bodies: &[(u32, &Pickable)],
    lines: &[&Strips],
    pointer: [f64; 2],
) -> Option<(Caster, Line)> {
    let ppp = pixels_per_point;
    if !(ppp.is_finite() && ppp > 0.0 && ppp <= MAX_PIXELS_PER_POINT) {
        return None;
    }
    let inside = pointer
        .iter()
        .zip(size_px)
        .all(|(c, side)| c.is_finite() && *c >= 0.0 && *c < f64::from(side));
    if !inside {
        return None;
    }
    let caster = Caster::new(camera, size_px, f64::from(ppp), bodies, lines)?;
    let (origin, direction) = ray(camera, size_px, pointer)?;
    let line = Line {
        origin,
        direction,
        whole: !caster.projector.perspective,
    };
    Some((caster, line))
}

/// A hit as [`hits`] orders it: its depth, less its reach for an edge or a vertex.
struct Found {
    hit: Hit,
    key: f64,
}

/// An element's place among hits at one depth: a vertex, then an edge, then a face.
fn rank(element: Element) -> u8 {
    match element {
        Element::Vertex(_) => 0,
        Element::Edge(_) => 1,
        Element::Face(_) => 2,
    }
}

/// How far past a triangle's edges, of the triangle (its barycentric coordinates), a line is
/// still taken to meet it: a part in 2^40, far under any pixel, so a line along an edge two
/// triangles share (a face's diagonal, or the edge between two faces, as the ray through an
/// edge's own point is) meets them whatever the rounding, never neither.
const WATERTIGHT: f64 = 1.0 / 1_099_511_627_776.0;

/// A ray, or, `whole`, the line it's on: its origin, and its direction of unit length.
struct Line {
    origin: [f64; 3],
    direction: [f64; 3],
    whole: bool,
}

impl Line {
    /// Whether it passes within `out` of the box `(low, high)` (the box grown by `out` on
    /// each side): the slabs' test, with a direction's 0 taken apart.
    fn meets_box(&self, (low, high): Bounds, out: f64) -> bool {
        let (mut enter, mut leave) = if self.whole {
            (f64::NEG_INFINITY, f64::INFINITY)
        } else {
            (0.0, f64::INFINITY)
        };
        for k in 0..3 {
            let (lo, hi) = (low[k] - out, high[k] + out);
            let (o, d) = (self.origin[k], self.direction[k]);
            if d == 0.0 {
                if !(lo <= o && o <= hi) {
                    return false;
                }
                continue;
            }
            let (a, b) = ((lo - o) / d, (hi - o) / d);
            enter = enter.max(a.min(b));
            leave = leave.min(a.max(b));
        }
        enter <= leave
    }

    /// Where it meets the triangle `[a, b, c]`, by how far along it, if it does, its edges
    /// and corners its own, and as far past them as [`WATERTIGHT`] (Möller and Trumbore's,
    /// in f64, either side facing it); none for a triangle seen edge on, which draws nothing,
    /// or one met past f64.
    fn meets(&self, [a, b, c]: [[f64; 3]; 3]) -> Option<f64> {
        let (e1, e2) = (sub(b, a), sub(c, a));
        let p = cross(self.direction, e2);
        let det = dot(e1, p);
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let s = sub(self.origin, a);
        let u = dot(s, p) / det;
        if !(-WATERTIGHT..=1.0 + WATERTIGHT).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = dot(self.direction, q) / det;
        if !(v >= -WATERTIGHT && u + v <= 1.0 + WATERTIGHT) {
            return None;
        }
        let t = dot(e2, q) / det;
        (t.is_finite() && (self.whole || t >= 0.0)).then_some(t)
    }
}

/// [`hits`]'s view: the renderer's projection ([`Projector`]), the eye and the way it looks,
/// the pixels a point, how much of the view a pixel is (mm, orthographic; mm for each mm of
/// depth, in perspective), how tall the view is (`span`: mm, orthographic; for each mm from
/// the eye, in perspective, and `rows`, in pixels), and, orthographic, how far in front of
/// the target the renderer draws the bodies from ([`orthographic_depth`]: its near plane).
struct Caster {
    projector: Projector,
    eye: [f64; 3],
    forward: [f64; 3],
    pixels_per_point: f64,
    per_pixel: f64,
    span: f64,
    rows: u32,
    near: Option<f64>,
}

impl Caster {
    /// The view, of `bodies` and `lines` as the renderer's view takes them (the bodies and
    /// lines a frame draws, by [`check_bodies_in_view`] and [`with_lines`], the rules
    /// [`Renderer::prepare`] takes: an orthographic camera set past them all); none for a
    /// camera the checks refuse, or bodies or lines its view does.
    fn new(
        camera: &Camera,
        size_px: [u32; 2],
        pixels_per_point: f64,
        bodies: &[(u32, &Pickable)],
        lines: &[&Strips],
    ) -> Option<Caster> {
        let projector = Projector::new(camera, size_px)?;
        let drawn = bodies.iter().map(|&(body, pickable)| InViewBody {
            body,
            mesh: &pickable.mesh,
            bounds: pickable.drawn,
        });
        let target = camera.target;
        let mut exact = |_: u32, mesh: &Mesh| reach_of_points(mesh, target);
        let in_view = check_bodies_in_view(camera, size_px[1], drawn, &mut exact).ok()?;
        let in_view =
            with_lines(camera, size_px[1], in_view, lines.iter().copied(), "lines").ok()?;
        let (span, near) = match in_view {
            InView::Orthographic { view_height, reach } => {
                (view_height, Some(orthographic_depth(reach)))
            }
            InView::Perspective { field_of_view } => {
                (2.0 * libm::tan(field_of_view.to_radians() / 2.0), None)
            }
        };
        let forward = projector.frame.back.map(|c| -c);
        Some(Caster {
            eye: camera.eye,
            forward,
            pixels_per_point,
            per_pixel: span / f64::from(size_px[1]),
            span,
            rows: size_px[1],
            near,
            projector,
        })
    }

    /// `p`'s depth along the view, from the eye.
    fn depth(&self, p: [f64; 3]) -> f64 {
        dot(self.forward, sub(p, self.eye))
    }

    /// The depth of `point`, met `t` along `line`: in perspective every line cast is from
    /// the eye, and its depth is how far along it, in the view's direction, precise about
    /// the eye wherever the eye is; orthographic, [`depth`](Self::depth)'s.
    fn depth_on(&self, line: &Line, t: f64, point: [f64; 3]) -> f64 {
        if self.projector.perspective {
            t * dot(line.direction, self.forward)
        } else {
            self.depth(point)
        }
    }

    /// The least depth anything in the box `bounds` is met at by a line cast (from the eye,
    /// in perspective; through `line`'s origin, orthographic), rounding's too: its corner
    /// nearest the eye's (depth is linear, so a box's least is at a corner), less what that
    /// and a meeting's rounding can come to (a part in 2^38 of the largest coordinate the
    /// two are worked from, the triangles' own [`WATERTIGHT`] among it).
    fn nearest_depth(&self, (low, high): Bounds, line: &Line) -> f64 {
        let corner: [f64; 3] = std::array::from_fn(|k| {
            if self.forward[k] >= 0.0 {
                low[k]
            } else {
                high[k]
            }
        });
        let largest = low
            .iter()
            .chain(&high)
            .chain(&self.eye)
            .chain(&line.origin)
            .map(|c| c.abs())
            .fold(1.0, f64::max);
        self.depth(corner) - largest * f64::EPSILON * 16384.0
    }

    /// A vertex's reach toward the eye at `point`, `depth` along the view: a frame's picking
    /// disc's there ([`vertex_reach`]).
    fn vertex_reach_at(&self, point: [f64; 3], depth: f64) -> f64 {
        let (perspective, ahead) = self.ahead_of(point, depth);
        vertex_reach(perspective, self.span, ahead)
    }

    /// An edge's reach toward the eye at `point`, `depth` along the view: a frame's picking
    /// strip's there ([`edge_reach`]).
    fn edge_reach_at(&self, point: [f64; 3], depth: f64) -> f64 {
        let (perspective, ahead) = self.ahead_of(point, depth);
        edge_reach(perspective, self.span, self.rows, ahead)
    }

    /// Whether the view is in perspective, and how far in front of the renderer's camera
    /// `point`, `depth` along the view, is (orthographic, its near plane,
    /// [`orthographic_depth`] in front of the target; in perspective, the eye): what a reach
    /// is of.
    fn ahead_of(&self, point: [f64; 3], depth: f64) -> (bool, f64) {
        match self.near {
            Some(near) => {
                let along = dot(self.projector.frame.back, sub(point, self.projector.origin));
                (false, near - along)
            }
            None => (true, depth),
        }
    }

    /// Whether a point at `depth` is drawn: in perspective, no nearer the eye than
    /// [`MIN_EYE_DISTANCE`]; orthographic, anywhere.
    fn drawn(&self, depth: f64) -> bool {
        depth.is_finite() && (!self.projector.perspective || depth >= MIN_EYE_DISTANCE)
    }

    /// How far out a box is grown so that whatever in it lands within `pixels` of a pixel
    /// lies within it of that pixel's ray: the pixels' width at the box's farthest depth, and
    /// a little of the box's size more, for rounding; none for a box nothing of which is
    /// drawn.
    fn grown(&self, bounds: Bounds, pixels: f64) -> Option<f64> {
        let slack = slack_of(bounds);
        if !self.projector.perspective {
            return Some(pixels * self.per_pixel + slack);
        }
        let farthest = box_corners(bounds)
            .into_iter()
            .map(|corner| self.depth(corner))
            .fold(f64::NEG_INFINITY, f64::max);
        (farthest >= MIN_EYE_DISTANCE).then_some(pixels * self.per_pixel * farthest + slack)
    }

    /// Each face of `pickable` `line` meets, at its nearest drawn meeting: its tree walked
    /// once, from its root (the body's box), each face's nearest kept.
    fn faces_met(&self, body: u32, pickable: &Pickable, line: &Line, found: &mut Vec<Found>) {
        let mut nearest: BTreeMap<u32, ([f64; 3], f64)> = BTreeMap::new();
        pickable.triangles.walk(
            self,
            &pickable.mesh,
            line,
            |_| true,
            |face, point, depth| {
                let held = nearest.entry(face).or_insert((point, depth));
                if depth < held.1 {
                    *held = (point, depth);
                }
                false
            },
        );
        for (face, (point, depth)) in nearest {
            found.push(Found {
                hit: Hit {
                    body,
                    element: Element::Face(face),
                    point,
                    depth,
                    hidden: false,
                },
                key: depth,
            });
        }
    }

    /// Each edge of `pickable` whose drawn polyline passes within [`HIT_EDGE_POINTS`] of
    /// `pointer`, at its point nearest it: only of the boxes that come within that of the
    /// pointer's `line`, grown by it at their farthest depth (the body's, then the edge's),
    /// each point of an edge placed and projected once.
    fn edges_near(
        &self,
        body: u32,
        pickable: &Pickable,
        line: &Line,
        pointer: [f64; 2],
        found: &mut Vec<Found>,
    ) {
        let pixels = HIT_EDGE_POINTS * self.pixels_per_point;
        let near = |bounds: Bounds| {
            self.grown(bounds, pixels)
                .is_some_and(|out| line.meets_box(bounds, out))
        };
        if !pickable.bounds.is_some_and(near) {
            return;
        }
        let edges = edge_spans(&pickable.mesh).zip(&pickable.edges).zip(1u32..);
        for ((points, bounds), number) in edges {
            if !bounds.is_some_and(near) {
                continue;
            }
            let mut best: Option<(f64, [f64; 3], f64)> = None;
            let mut last: Option<EdgePoint> = None;
            for &at in points {
                let here = self.edge_point(at);
                if let Some(before) = &last
                    && let Some((distance, point, depth)) = self.nearest_on(before, &here, pointer)
                {
                    let better =
                        best.is_none_or(|(d, _, z)| distance < d || (distance == d && depth < z));
                    if better {
                        best = Some((distance, point, depth));
                    }
                }
                last = Some(here);
            }
            if let Some((distance, point, depth)) = best
                && distance < pixels
            {
                found.push(Found {
                    hit: Hit {
                        body,
                        element: Element::Edge(number),
                        point,
                        depth,
                        hidden: false,
                    },
                    key: depth - self.edge_reach_at(point, depth),
                });
            }
        }
    }

    /// An edge's point `at` as [`nearest_on`](Self::nearest_on) takes it: about the camera's
    /// point, whether the drawing keeps it (in perspective, at [`MIN_EYE_DISTANCE`] or more
    /// along the view, by the drawing's own rule; orthographic, always), and where it lands
    /// if it's kept.
    fn edge_point(&self, at: [f64; 3]) -> EdgePoint {
        let view = self.projector.view_of(at);
        let kept = !self.projector.perspective || -view.z - MIN_EYE_DISTANCE >= 0.0;
        EdgePoint {
            at,
            view,
            kept,
            landed: if kept {
                self.projector.landing(view)
            } else {
                None
            },
        }
    }

    /// Of the segment from `a` to `b`, drawn (cut where the drawing cuts it, in 3D), its
    /// point nearest `pointer` in the image: how far from it, in pixels, the point, and its
    /// depth; of a segment seen end on, its end nearest the eye. None if nothing of it is
    /// drawn, or it lands past f64. Cut, projected, and measured about the camera's point (in
    /// perspective, the eye), as the drawing cuts and places its strips (`View::cut_strip`,
    /// [`project_segment`]): a point near the eye is precise about it wherever the eye is, as
    /// it isn't in the document's coordinates far from their origin.
    fn nearest_on(
        &self,
        a: &EdgePoint,
        b: &EdgePoint,
        pointer: [f64; 2],
    ) -> Option<(f64, [f64; 3], f64)> {
        let (va, vb) = (a.view, b.view);
        let on = |u: f64| va + (vb - va) * u;
        let (enter, leave) = if self.projector.perspective {
            Projector::near_cut(va, vb)?
        } else {
            (0.0, 1.0)
        };
        let (near, (pa, wa)) = if a.kept {
            (va, a.landed?)
        } else {
            let v = on(enter);
            (v, self.projector.landing(v)?)
        };
        let (far, (pb, wb)) = if b.kept {
            (vb, b.landed?)
        } else {
            let v = on(leave);
            (v, self.projector.landing(v)?)
        };
        let d = [pb[0] - pa[0], pb[1] - pa[1]];
        let length = d[0] * d[0] + d[1] * d[1];
        let s = if length == 0.0 {
            // Seen end on: its end nearest the eye (about the camera, the greater z).
            if far.z > near.z { 1.0 } else { 0.0 }
        } else {
            let along_image = (pointer[0] - pa[0]) * d[0] + (pointer[1] - pa[1]) * d[1];
            (along_image / length).clamp(0.0, 1.0)
        };
        let at = [pa[0] + d[0] * s, pa[1] + d[1] * s];
        let distance = image_distance(at, pointer);
        // The point in 3D whose image is there: the image's fraction taken back through the
        // perspective division (by each end's w, its depth); orthographic, the same fraction.
        let t = if self.projector.perspective {
            let w = (1.0 - s) * wb + s * wa;
            if w == 0.0 { s } else { s * wa / w }
        } else {
            s
        };
        let u = (enter + (leave - enter) * t.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let point = along(a.at, b.at, u);
        let depth = if self.projector.perspective {
            -on(u).z
        } else {
            self.depth(point)
        };
        // Drawn: what's left of it is, cut where the drawing cuts it (a cut end's depth may
        // round a hair under the cut, and is drawn all the same).
        (distance.is_finite() && depth.is_finite() && point.iter().all(|c| c.is_finite()))
            .then_some((distance, point, depth))
    }

    /// Each vertex of `pickable` drawn within [`HIT_VERTEX_POINTS`] of `pointer`: none if
    /// its body's box doesn't come within that of the pointer's `line`.
    fn vertices_near(
        &self,
        body: u32,
        pickable: &Pickable,
        line: &Line,
        pointer: [f64; 2],
        found: &mut Vec<Found>,
    ) {
        let pixels = HIT_VERTEX_POINTS * self.pixels_per_point;
        let near = pickable.bounds.is_some_and(|bounds| {
            self.grown(bounds, pixels)
                .is_some_and(|out| line.meets_box(bounds, out))
        });
        if !near {
            return;
        }
        for (point, number) in pickable.mesh.vertex_points.iter().zip(1u32..) {
            let Some(at) = self.projector.pixel(*point) else {
                continue;
            };
            let distance = image_distance(at, pointer);
            let depth = self.depth(*point);
            if distance < pixels && self.drawn(depth) {
                found.push(Found {
                    hit: Hit {
                        body,
                        element: Element::Vertex(number),
                        point: *point,
                        depth,
                        hidden: false,
                    },
                    key: depth - self.vertex_reach_at(*point, depth),
                });
            }
        }
    }

    /// Whether a face of `bodies` is met, on the ray through `found`'s own point, nearer the
    /// eye than its depth less its reach (its key): each body's tree walked from its root
    /// (the body's box), past every box wholly no nearer than that, and stopped at the first
    /// triangle met nearer.
    fn hidden(&self, bodies: &[(u32, &Pickable)], found: &Found) -> bool {
        let point = found.hit.point;
        let line = if self.projector.perspective {
            let toward = sub(point, self.eye);
            let length = dot(toward, toward).sqrt();
            if !(length.is_finite() && length > 0.0) {
                return false;
            }
            Line {
                origin: self.eye,
                direction: toward.map(|c| c / length),
                whole: false,
            }
        } else {
            Line {
                origin: point,
                direction: self.forward,
                whole: true,
            }
        };
        let below = found.key;
        bodies.iter().any(|(_, pickable)| {
            pickable.triangles.walk(
                self,
                &pickable.mesh,
                &line,
                |bounds| self.nearest_depth(bounds, &line) < below,
                |_, _, depth| depth < below,
            )
        })
    }
}

/// An edge's point as [`Caster::nearest_on`] takes it ([`Caster::edge_point`]): its own, in
/// the document; about the camera's point; whether the drawing keeps it; and where it lands
/// (with its clip w) if it's kept and that's finite.
struct EdgePoint {
    at: [f64; 3],
    view: glam::DVec4,
    kept: bool,
    landed: Option<([f64; 2], f64)>,
}

/// What a view from `camera`, `size_px` in pixels, draws of the segment from `a` to `b` (in
/// the document's coordinates): the pixels its drawn part's two ends land on, as [`project`]
/// places a point, each none where its pixel isn't finite; or none where nothing of it is
/// drawn. In perspective it's cut where the drawing cuts it, at [`MIN_EYE_DISTANCE`] along
/// the view from the eye, by the drawing's own rule ([`cut_by`]), the cut taken about the
/// eye in f64: so where it's cut is as precise as the eye is, wherever the eye is (taken in
/// the document's coordinates, far from their origin, a point a nanometre from the eye is
/// off by their rounding, which lands it pixels away). Orthographic, it's drawn whole. The
/// pyramid's other sides, 2^19 pixels off the view's axis, cut nothing within the image.
/// None too for a camera [`Camera::check`] refuses at this size, a size of 0 on a side, or
/// an end that isn't finite.
pub fn project_segment(
    camera: &Camera,
    size_px: [u32; 2],
    a: [f64; 3],
    b: [f64; 3],
) -> Option<[Option<[f64; 2]>; 2]> {
    SegmentProjection::new(camera, size_px)?
        .segment(a, b)
        .map(|drawn| drawn.pixels)
}

/// [`project_segment`]'s projection of one view, made once for many segments (the app's
/// ground grid's, each frame).
pub struct SegmentProjection {
    projector: Projector,
}

/// What a view draws of a segment ([`SegmentProjection::segment`]): where along it, from its
/// first end, its drawn part begins and ends (0 and 1 for the whole of it), and the pixels
/// those two land on, each none where its pixel isn't finite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnSegment {
    pub along: [f64; 2],
    pub pixels: [Option<[f64; 2]>; 2],
}

impl SegmentProjection {
    /// The projection of a view from `camera`, `size_px` in pixels; none for a camera
    /// [`Camera::check`] refuses at this size, or a size of 0 on a side.
    pub fn new(camera: &Camera, size_px: [u32; 2]) -> Option<SegmentProjection> {
        Some(SegmentProjection {
            projector: Projector::new(camera, size_px)?,
        })
    }

    /// What it draws of the segment from `a` to `b`, as [`project_segment`] says: in
    /// perspective cut at [`MIN_EYE_DISTANCE`] along the view from the eye, orthographic
    /// whole; none where nothing of it is drawn, or an end isn't finite.
    pub fn segment(&self, a: [f64; 3], b: [f64; 3]) -> Option<DrawnSegment> {
        if !a.iter().chain(&b).all(|c| c.is_finite()) {
            return None;
        }
        let projector = &self.projector;
        let (va, vb) = (projector.view_of(a), projector.view_of(b));
        let (enter, leave) = if projector.perspective {
            Projector::near_cut(va, vb)?
        } else {
            (0.0, 1.0)
        };
        // An end the drawing keeps lands as `project` lands it; one it cuts, where it's cut.
        let end = |v: glam::DVec4, kept: bool, at: f64| {
            let v = if kept { v } else { va + (vb - va) * at };
            projector.landing(v).map(|(pixel, _)| pixel)
        };
        let kept = |v: glam::DVec4| !projector.perspective || -v.z - MIN_EYE_DISTANCE >= 0.0;
        Some(DrawnSegment {
            along: [enter, leave],
            pixels: [end(va, kept(va), enter), end(vb, kept(vb), leave)],
        })
    }
}

/// What of a segment lies on the inner side of a plane, by the plane's linear form at its
/// two ends (`fa`, `fb`, at least 0 inside), narrowed from `kept` (the fractions along it,
/// from `a`, it enters and leaves at): what's left, or none if nothing is. The drawing's one
/// rule for a cut ([`View::cut_segment`], each side of its pyramid in turn), and the near
/// cut Select Other's cast and box selection make of an edge (its pyramid's near side,
/// [`Projector::near_cut`], [`project_segment`]): so what they take is what's drawn.
fn cut_by(fa: f64, fb: f64, (enter, leave): (f64, f64)) -> Option<(f64, f64)> {
    let (enter, leave) = match (fa >= 0.0, fb >= 0.0) {
        (true, true) => (enter, leave),
        (false, false) => return None,
        (true, false) => (enter, leave.min(fa / (fa - fb))),
        (false, true) => (enter.max(fa / (fa - fb)), leave),
    };
    (enter <= leave).then_some((enter, leave))
}

/// How far toward the eye a vertex's picking disc is drawn ([`View::disc_at`]), and Select
/// Other's cast takes a vertex ([`hits`]), of a point `ahead` mm in front of the drawing's
/// camera (orthographic, its near plane, at the camera; in perspective, the eye, along the
/// view): [`VERTEX_REACH`] of the view's height there, the prototype's `pickAll` tolerance
/// ([`reach`]'s caps).
///
/// Its limit, accepted for M5 (N5, decided 2026-11-08): the reach is depth alone, so a vertex
/// behind a wall thinner than about 1.2% of the view's height (1.2 mm in a view 100 mm high)
/// can be picked through it, and Select Other lists it unhidden. It can't be less: the disc
/// must win its rim, 7 points from the vertex, against the faces it bounds that rise toward
/// the eye. The pick shows on hover before a click and can be undone. The better rule, after
/// M5 (docs/roadmap/m5-app.md), is by adjacency: a vertex's pick dropped where a face nearer
/// than an edge's reach ([`edge_reach`]) covers it that isn't one of the vertex's own.
fn vertex_reach(perspective: bool, span: f64, ahead: f64) -> f64 {
    reach(VERTEX_REACH, perspective, span, ahead)
}

/// How far toward the eye an edge's picking strip is drawn ([`edge_picks`]), and Select
/// Other's cast takes an edge ([`hits`]), of a point `ahead` mm in front of the drawing's
/// camera in a view `rows` pixels high: [`EDGE_REACH_PIXELS`] of the view's depth there
/// ([`edge_fraction`] of its height), with [`vertex_reach`]'s caps, so never more than it.
fn edge_reach(perspective: bool, span: f64, rows: u32, ahead: f64) -> f64 {
    reach(edge_fraction(rows), perspective, span, ahead)
}

/// The fraction of a view's height, `rows` pixels, that an edge's reach is: [`EDGE_REACH_PIXELS`]
/// of its rows, and at most [`VERTEX_REACH`] (which it is under 167 rows, and for a view of
/// none).
fn edge_fraction(rows: u32) -> f64 {
    (EDGE_REACH_PIXELS / f64::from(rows)).min(VERTEX_REACH)
}

/// How far toward the eye a drawn edge's strip is moved ([`edges`]), in a view `rows` pixels
/// high at `pixels_per_point`: of the view's height, by [`reach`]'s caps, as an edge's picking
/// strip is by [`edge_fraction`], and never more than it (N5's image). An edge's stroke is
/// drawn square to the view at its line's depth, half its width either side of its line, so a
/// face beside it that rises toward the eye across it hides the part of its stroke nearer the
/// eye than its line: at a concave edge both faces do, and at a fillet's tangent edge the flat
/// face does. Moved by `R` pixels of the view's depth, a stroke `h` pixels from its line to
/// its side ([`EDGE_WIDTH`]'s half, `h` = `pixels_per_point`) is drawn whole over a face rising
/// by up to `g` pixels of depth a pixel where `g h < R`, and over a curved face's mesh, whose
/// chords lie up to [`MESH_SAGITTA_PIXELS`] of depth nearer the eye than the surface its line
/// lies on, where `g h + 1/2 < R`. So the least that draws it whole over a face turned up to
/// 54.7° from face-on (`g` = [`DRAWN_EDGE_SLOPE`], √2) meshed as the view is: `R` = √2 `h` +
/// 1/2, 1.91 pixels at a pixel a point. Over 1.06 pixels a point that's more than an edge's
/// picking reach ([`EDGE_REACH_PIXELS`], 2 pixels), and it's held to that: from there an
/// edge's stroke is drawn whole over a face rising by up to (2 − 1/2)/`h` (at 2 pixels a
/// point, 0.75, 37°; a flat face to 1, 45°), and over a steeper one to 2/`g` pixels of its
/// line. Its cost is an edge's picking's: a wall less than `R` of depth thick in front of an
/// edge doesn't hide it (0.239 mm in a view 100 mm high on 800 rows), and an edge going
/// behind a face shows a stub where it's within `R` of it.
fn drawn_edge_fraction(rows: u32, pixels_per_point: f32) -> f64 {
    drawn_stroke_fraction(rows, EDGE_WIDTH, pixels_per_point)
}

/// [`drawn_edge_fraction`]'s rule for a stroke `width` points wide: a frame's line, drawn
/// over a face it lies on or along an edge as an edge is, moved toward the eye by the reach
/// that draws its stroke whole there, held to an edge's picking reach as an edge's is.
fn drawn_stroke_fraction(rows: u32, width: f32, pixels_per_point: f32) -> f64 {
    let half = f64::from(width) / 2.0 * f64::from(pixels_per_point);
    let pixels = DRAWN_EDGE_SLOPE * half + MESH_SAGITTA_PIXELS;
    (pixels / f64::from(rows)).min(edge_fraction(rows))
}

/// `fraction` of a view's height at a point `ahead` mm in front of the drawing's camera, as a
/// reach toward the eye. Orthographic, `span` is the view's height, and the reach at most
/// halfway to the near plane, as little as a millimetre in front of the scene, so a disc or a
/// strip at the scene's front isn't cut; in perspective, `span` is the view's height a
/// millimetre from the eye, and the reach at most [`MOST_REACH`] of the depth, so a
/// view as wide as 180° keeps it ahead of the eye: in mm along the view, `ahead` times that
/// fraction (at an `ahead` of 1, the fraction itself, which a point is moved along its line to
/// the eye by).
fn reach(fraction: f64, perspective: bool, span: f64, ahead: f64) -> f64 {
    if perspective {
        (fraction * span).min(MOST_REACH) * ahead
    } else {
        (fraction * span).min(ahead / 2.0)
    }
}

/// `v`, a point about the drawing's camera (x right, y up, z toward the eye:
/// [`View::about_camera`]'s), moved toward the eye by its [`reach`] of `fraction` of a view
/// `span` high, in f64: along the view (orthographic, `−z` ahead of the camera, at z = 0) or
/// along its line to the eye (perspective, the eye at the origin), so it lands on the same
/// pixel.
fn toward_eye(v: [f64; 3], fraction: f64, perspective: bool, span: f64) -> [f64; 3] {
    if perspective {
        let toward = reach(fraction, true, span, 1.0);
        v.map(|c| c * (1.0 - toward))
    } else {
        [v[0], v[1], v[2] + reach(fraction, false, span, -v[2])]
    }
}

/// [`toward_eye`] undone: the point `v` is where it moves to. Orthographic, a point is moved
/// by the whole reach where that's at most halfway to the camera, so to at most the reach in
/// front of it (z ≤ −reach), and by half its way otherwise, halving z; in perspective it's
/// scaled about the eye by at least 3/4 ([`MOST_REACH`]).
fn back_from_eye(v: [f64; 3], fraction: f64, perspective: bool, span: f64) -> [f64; 3] {
    if perspective {
        let toward = reach(fraction, true, span, 1.0);
        v.map(|c| c / (1.0 - toward))
    } else {
        let whole = fraction * span;
        let z = if v[2] <= -whole {
            v[2] - whole
        } else {
            2.0 * v[2]
        };
        [v[0], v[1], z]
    }
}

/// How far in front of an orthographic view's target its camera is set, past every point of
/// the bodies (their `reach` from the target, as [`check_bodies_in_view`] finds it):
/// [`CLEARANCE`] of the reach more, and at least a millimetre. Its near plane is there (the
/// camera's), and its far plane as far past the target.
fn orthographic_depth(reach: f64) -> f64 {
    reach + (reach * CLEARANCE).max(1.0)
}

/// How far apart two points of the image are, in pixels.
fn image_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

/// The point `s` of the way from `a` to `b`.
fn along(a: [f64; 3], b: [f64; 3], s: f64) -> [f64; 3] {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * s)
}

/// A triangle's corners, if each is one of `mesh`'s positions.
fn corners_of(mesh: &Mesh, triangle: &[u32; 3]) -> Option<[[f64; 3]; 3]> {
    let corner = |v: u32| mesh.positions.get(usize::try_from(v).ok()?).copied();
    Some([
        corner(triangle[0])?,
        corner(triangle[1])?,
        corner(triangle[2])?,
    ])
}

/// How far a box is grown for rounding alone: a part in 2^40 of its largest coordinate (and
/// of a millimetre), so a test of the box never leaves out what the test of what's in it
/// would take.
fn slack_of((low, high): Bounds) -> f64 {
    let largest = low.iter().chain(&high).map(|c| c.abs()).fold(1.0, f64::max);
    largest * f64::EPSILON * 4096.0
}

/// Where a mesh stored about `centre` is drawn, for the camera at `origin` (then `depth`
/// back along the view) with `frame`: the view's rotation (its right, up, and back as the
/// world's x, y, and z, where the fixed camera looks along −z with +y up), and the centre
/// about the camera in the camera's frame, `R (centre − origin) − (0, 0, depth)`, in f64
/// and then f32. A face far from the origin keeps its precision: the offset is taken in f64
/// before anything is f32. What's f32 about its centre is the face's size, which a close
/// enough view of a large face would see: that one is drawn about the camera's point instead
/// ([`View::anchor`]).
pub fn world_from_mesh(
    frame: &CameraFrame,
    origin: [f64; 3],
    depth: f64,
    centre: [f64; 3],
) -> glam::Affine3A {
    let d = sub(centre, origin);
    let rows = [frame.right, frame.up, frame.back];
    let translation = [dot(rows[0], d), dot(rows[1], d), dot(rows[2], d) - depth];
    let column = |k: usize| vec3([rows[0][k], rows[1][k], rows[2][k]]);
    glam::Affine3A::from_mat3_translation(
        glam::Mat3::from_cols(column(0), column(1), column(2)),
        vec3(translation),
    )
}

/// In f32 on purpose: re_renderer draws in f32, after the offsets are taken in f64.
fn vec3(a: [f64; 3]) -> glam::Vec3 {
    glam::Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32)
}

/// An sRGB colour as re_renderer's linear one.
fn srgb([r, g, b]: [u8; 3]) -> Rgba {
    Rgba::from_srgba_unmultiplied(r, g, b, 255)
}

/// The body's colour as a vertex's, plain or tinted with the highlight: in linear light, half
/// the body's and half the blue, then sRGB (an `Rgba`'s alpha must stay 1, or the
/// conversion premultiplies in sRGB). re_renderer's light then shades it.
fn vertex_colour(highlighted: bool) -> Rgba32Unmul {
    if !highlighted {
        let [r, g, b] = BODY;
        return Rgba32Unmul::from_rgba_unmul_array([r, g, b, 255]);
    }
    let (body, blue) = (srgb(BODY), srgb(HIGHLIGHT));
    let mix = |a: f32, b: f32| a * (1.0 - HIGHLIGHT_STRENGTH) + b * HIGHLIGHT_STRENGTH;
    let c = Color32::from(Rgba::from_rgb(
        mix(body.r(), blue.r()),
        mix(body.g(), blue.g()),
        mix(body.b(), blue.b()),
    ));
    Rgba32Unmul::from_rgba_unmul_array([c.r(), c.g(), c.b(), 255])
}

/// A normal of unit length, made so in f64, where a huge or tiny f32 one neither overflows
/// nor underflows (re_renderer normalizes in f32, where it would: f32's largest squared is
/// 1.2e77 and its least 2e-90, both well inside f64's range); none for zero, which
/// re_renderer draws unlit.
fn unit(n: [f32; 3]) -> glam::Vec3 {
    let n = n.map(f64::from);
    let length = dot(n, n).sqrt();
    if length == 0.0 {
        return glam::Vec3::ZERO;
    }
    vec3(n.map(|c| c / length))
}

/// A body as uploaded: what it was made from, to know it again, draw its edges from, and
/// check a frame's view of it by; its box (its positions' and edge points', none for a mesh
/// of neither), whose corners a frame's view is checked by first; and its faces.
struct Uploaded {
    mesh: Arc<Mesh>,
    /// Which of the renderer's meshes it is ([`Renderer::next_generation`]); 0 until the
    /// renderer gives it one.
    generation: u64,
    bounds: Option<Bounds>,
    /// Each edge's points' box, in the mesh's order (none for an edge of none).
    edge_bounds: Vec<Option<Bounds>>,
    faces: Vec<UploadedFace>,
}

/// What of a body is highlighted, what hovered, and what tinted (a frame's): its faces'
/// and edges' numbers from 1, ascending, each once; and how it's highlighted (a hover is
/// always outlined).
#[derive(Default)]
struct Lit {
    faces: Vec<u32>,
    edges: Vec<u32>,
    hovered_faces: Vec<u32>,
    hovered_edges: Vec<u32>,
    tinted_faces: Vec<(u32, Tint)>,
    style: Highlight,
}

impl Lit {
    /// A scene's body's highlights, which it numbers from 0.
    fn of_body(body: &RenderBody) -> Lit {
        let numbers = |from_0: &[u32]| from_0.iter().map(|i| i.saturating_add(1)).collect();
        Lit {
            faces: numbers(&body.highlight_faces),
            edges: numbers(&body.highlight_edges),
            style: Highlight::Baked,
            ..Lit::default()
        }
    }

    fn face(&self, number: u32) -> bool {
        self.faces.binary_search(&number).is_ok()
    }

    fn edge(&self, number: u32) -> bool {
        self.edges.binary_search(&number).is_ok()
    }

    fn hovered_face(&self, number: u32) -> bool {
        self.hovered_faces.binary_search(&number).is_ok()
    }

    fn hovered_edge(&self, number: u32) -> bool {
        self.hovered_edges.binary_search(&number).is_ok()
    }

    /// Face `number`'s tint, if it's tinted.
    fn tint(&self, number: u32) -> Option<Tint> {
        self.tinted_faces
            .binary_search_by_key(&number, |&(face, _)| face)
            .ok()
            .and_then(|at| self.tinted_faces.get(at))
            .map(|&(_, tint)| tint)
    }

    /// Whether anything of the body is highlighted or hovered: outlined (a tint isn't).
    fn any(&self) -> bool {
        [
            &self.faces,
            &self.edges,
            &self.hovered_faces,
            &self.hovered_edges,
        ]
        .iter()
        .any(|numbers| !numbers.is_empty())
    }
}

/// A face with triangles: which it is, its box (its triangles' positions'), the point its
/// mesh is about ([`View::anchor`]), and its mesh, plain or baked, as the last render drew
/// it, each uploaded the first time a render draws the face so about that point.
struct UploadedFace {
    span: FaceSpan,
    bounds: Bounds,
    anchor: [f64; 3],
    /// The pyramid it was cut to ([`View::cut_key`]), where its box isn't wholly in the
    /// view's: then its mesh is its triangles cut to that pyramid, about the eye.
    cut: Option<[u64; 13]>,
    /// Whether it's placed in the last view settled ([`View::place`]): a face that can have
    /// no pixel there, or that the pyramid holds none of, isn't, and keeps its meshes.
    placed: bool,
    plain: Option<Arc<GpuMesh>>,
    baked: Option<Arc<GpuMesh>>,
    /// Its plain mesh drawn with a tint's material ([`Mixes::mixed`]), by the tint's
    /// amount's bits: kept while it's tinted so, so a tint held allocates nothing a frame.
    mixed: Option<(u32, Arc<GpuMesh>)>,
}

/// A face's number from 1, and its `count` triangles' place, from `first`, in its body's
/// mesh.
#[derive(Debug, Clone, Copy)]
struct FaceSpan {
    number: u32,
    first: usize,
    count: usize,
}

impl Uploaded {
    /// `mesh`'s faces with triangles, each about its box's centre, none uploaded yet.
    fn new(mesh: Arc<Mesh>) -> Result<Uploaded, String> {
        let mut faces = Vec::new();
        let mut first = 0usize;
        for (face, &count) in mesh.face_triangles.iter().enumerate() {
            let count = usize::try_from(count).map_err(|e| e.to_string())?;
            let at = first;
            first += count;
            let span = FaceSpan {
                number: u32::try_from(face + 1).map_err(|e| e.to_string())?,
                first: at,
                count,
            };
            // A face with no triangles has nothing to draw.
            let Some(bounds) = face_bounds(&mesh, span) else {
                continue;
            };
            faces.push(UploadedFace {
                span,
                bounds,
                anchor: centre_of(bounds),
                cut: None,
                placed: false,
                plain: None,
                mixed: None,
                baked: None,
            });
        }
        Ok(Uploaded {
            generation: 0,
            bounds: box_of(mesh.positions.iter().chain(&mesh.edge_points)),
            edge_bounds: edge_spans(&mesh)
                .map(|points| box_of(points.iter()))
                .collect(),
            mesh,
            faces,
        })
    }

    /// Its faces' GPU meshes dropped, to be uploaded again when they're next drawn.
    fn drop_gpu_meshes(&mut self) {
        for face in &mut self.faces {
            face.plain = None;
            face.baked = None;
            face.mixed = None;
        }
    }

    /// Each face's point for `view`, and what it won't draw dropped: its meshes about
    /// another point, and the colour it isn't drawn in (baked where `lit` highlights it
    /// in [`Highlight::Baked`], plain otherwise).
    fn settle(&mut self, view: &View, lit: &Lit) {
        let mesh = &self.mesh;
        for face in &mut self.faces {
            let span = face.span;
            let vertices = || -> Box<dyn Iterator<Item = [f64; 3]> + '_> {
                Box::new(
                    mesh.triangles[span.first..span.first + span.count]
                        .iter()
                        .flatten()
                        .filter_map(|&v| mesh.positions.get(usize::try_from(v).ok()?).copied()),
                )
            };
            // A face is cut to a perspective view's pyramid only where its own corners
            // cross it, about the eye, and cut again for each view that moves the pyramid;
            // one not placed keeps what it holds, for when a view brings it back.
            let placed = view.place(&view.frustum, face.bounds, vertices);
            face.placed = placed != Placed::Not;
            let (anchor, cut) = match placed {
                Placed::Not => continue,
                Placed::Cut => (view.origin, Some(view.cut_key())),
                Placed::Whole => (view.anchor(face.bounds), None),
            };
            if anchor != face.anchor || cut != face.cut {
                face.anchor = anchor;
                face.cut = cut;
                face.plain = None;
                face.baked = None;
                face.mixed = None;
            }
            if lit.style == Highlight::Baked && lit.face(face.span.number) {
                face.plain = None;
            } else {
                face.baked = None;
            }
        }
    }

    /// Each face as a mesh instance of its own, moved into `view` by its own transform, its
    /// picking ID the body's number `body` and the face's; a highlighted one (in `lit`)
    /// outlined, and its mesh baked in [`Highlight::Baked`]; a tinted one (a frame's) drawn
    /// in its tint, its mesh's vertices with `mixes`' material for the tint's amount, and the
    /// tint's colour added by its instance. A face's mesh is uploaded where it has none: how
    /// many were.
    fn faces(
        &mut self,
        ctx: &RenderContext,
        body: u32,
        lit: &Lit,
        mixes: &mut Mixes,
        view: &View,
        instances: &mut Vec<GpuMeshInstance>,
    ) -> Result<u64, String> {
        let mut uploads = 0;
        // The faces to be uploaded: each without the mesh, plain or baked, it's drawn with.
        let new = self
            .faces
            .iter()
            .filter(|f| f.placed && f.plain.is_none() && f.baked.is_none())
            .count();
        let _span = (new > 0).then(|| tracing::debug_span!("upload", body, faces = new).entered());
        for face in &mut self.faces {
            if !face.placed {
                continue;
            }
            let number = face.span.number;
            let highlighted = lit.face(number);
            let baked = lit.style == Highlight::Baked && highlighted;
            let held = if baked {
                &mut face.baked
            } else {
                &mut face.plain
            };
            let gpu_mesh = match held {
                Some(mesh) => Arc::clone(mesh),
                None => {
                    let vertices = match face.cut {
                        Some(_) => cut_face_vertices(&self.mesh, face.span, view)?,
                        None => face_vertices(&self.mesh, face.span, face.anchor)?,
                    };
                    // A face whose triangles meet the pyramid in less than an area (a corner
                    // or an edge of it) draws nothing in this view.
                    if vertices.triangles.is_empty() {
                        continue;
                    }
                    let cpu = face_cpu_mesh(ctx, body, face.span, vertices, baked)?;
                    let gpu = GpuMesh::new(ctx, &cpu).map_err(|e| format!("Face{number}: {e}"))?;
                    uploads += 1;
                    Arc::clone(held.insert(Arc::new(gpu)))
                }
            };
            // Opaque black, where it isn't tinted: re_renderer adds the tint's colour and
            // multiplies by its alpha, so this leaves the face's own colour.
            let (gpu_mesh, additive_tint) = match lit.tint(number).filter(|t| t.amount > 0.0) {
                Some(tint) => {
                    let bits = tint.amount.to_bits();
                    let mixed = match &face.mixed {
                        Some((held, mesh)) if *held == bits && !baked => Arc::clone(mesh),
                        _ => {
                            let mesh = mixes.mixed(ctx, &gpu_mesh, tint.amount)?;
                            if !baked {
                                face.mixed = Some((bits, Arc::clone(&mesh)));
                            }
                            mesh
                        }
                    };
                    (mixed, tint.added())
                }
                None => (gpu_mesh, Color32::BLACK),
            };
            instances.push(GpuMeshInstance {
                gpu_mesh,
                world_from_mesh: view.world_from_mesh(face.anchor),
                additive_tint,
                outline_mask_ids: outlined_as(highlighted, lit.hovered_face(number)),
                picking_layer_id: PickingLayerId {
                    object: PickingLayerObjectId(u64::from(body)),
                    instance: PickingLayerInstanceId(u64::from(number)),
                },
                cull_mode: None,
            });
        }
        Ok(uploads)
    }
}

/// The materials tinted faces are drawn with ([`Tint`]): for each tint's amount `a`, one
/// scaling a face's colour by `1 − a`, made once (as a material of a one-triangle mesh,
/// which `re_renderer` builds a material's bind group for) and given to a face's mesh, its
/// vertices and indices shared, so a tint uploads no face.
#[derive(Default)]
struct Mixes {
    /// By the amount's bits, the material's donor mesh.
    donors: BTreeMap<u32, Arc<GpuMesh>>,
}

impl Mixes {
    /// `face`, a face's mesh of one material, drawn with the colour it holds scaled by
    /// `1 − amount`: its vertices and indices, its material's index range, and the
    /// amount's material.
    fn mixed(
        &mut self,
        ctx: &RenderContext,
        face: &GpuMesh,
        amount: f32,
    ) -> Result<Arc<GpuMesh>, String> {
        let donor = match self.donors.get(&amount.to_bits()) {
            Some(donor) => Arc::clone(donor),
            None => {
                let kept = 1.0 - amount;
                let cpu = CpuMesh {
                    label: "tint".into(),
                    materials: smallvec::smallvec![Material {
                        label: "tint".into(),
                        index_range: re_renderer::Span { start: 0, len: 3 },
                        albedo: ctx.texture_manager_2d.white_texture_unorm_handle().clone(),
                        albedo_factor: Rgba::from_rgb(kept, kept, kept),
                    }],
                    triangle_indices: vec![glam::UVec3::new(0, 1, 2)],
                    vertex_positions: vec![glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y],
                    vertex_colors: vec![vertex_colour(false); 3],
                    vertex_normals: vec![glam::Vec3::Z; 3],
                    vertex_texcoords: vec![glam::Vec2::ZERO; 3],
                    bbox: macaw::BoundingBox::from_points(
                        [glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y].into_iter(),
                    ),
                };
                let donor = Arc::new(GpuMesh::new(ctx, &cpu).map_err(|e| format!("tint: {e}"))?);
                self.donors.insert(amount.to_bits(), Arc::clone(&donor));
                donor
            }
        };
        let (Some(own), Some(mix)) = (face.materials.first(), donor.materials.first()) else {
            return Err("a face's mesh or a tint's has no material".to_owned());
        };
        Ok(Arc::new(GpuMesh {
            materials: smallvec::smallvec![GpuMaterial {
                index_range: own.index_range,
                ..mix.clone()
            }],
            ..face.clone()
        }))
    }
}

/// Each edge's points of `mesh`, in its order: the counts the check makes add up.
fn edge_spans(mesh: &Mesh) -> impl Iterator<Item = &[[f64; 3]]> {
    let mut first = 0usize;
    mesh.edge_point_counts.iter().map(move |&count| {
        // As a usize, at most the points'.
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        let points = mesh
            .edge_points
            .get(first..first.saturating_add(count))
            .unwrap_or(&[]);
        first = first.saturating_add(count);
        points
    })
}

/// Each edge of `uploaded`'s mesh a strip can draw, in its order: its number from 1 (the ID
/// buffer's and its name's, `Edge12`), its points, two or more, and its box. None past a
/// u32's numbers. The drawn edges ([`edges`]) and their picking strips ([`edge_picks`]) are
/// numbered and found by it alike.
fn strip_edges(uploaded: &Uploaded) -> impl Iterator<Item = (u32, &[[f64; 3]], Bounds)> {
    edge_spans(&uploaded.mesh)
        .zip(&uploaded.edge_bounds)
        .enumerate()
        .map_while(|(edge, pair)| Some((u32::try_from(edge + 1).ok()?, pair)))
        .filter_map(|(number, (points, bounds))| {
            let bounds = (*bounds)?;
            (points.len() >= 2).then_some((number, points, bounds))
        })
}

/// Body `body`'s batch of edge strips, labelled `label`: its strips' points placed in the
/// view already (so the identity), its picking object the body's, at [`EDGE_DEPTH_OFFSET`].
fn edge_batch<'a, 'ctx>(
    lines: &'a mut LineDrawableBuilder<'ctx>,
    body: u32,
    label: String,
) -> re_renderer::LineBatchBuilder<'a, 'ctx> {
    lines
        .batch(label)
        .world_from_obj(glam::Affine3A::IDENTITY)
        .picking_object_id(PickingLayerObjectId(u64::from(body)))
        .depth_offset(EDGE_DEPTH_OFFSET)
}

/// Edge `number`'s picking instance: its number with [`EDGE_BIT`].
fn edge_instance(number: u32) -> PickingLayerInstanceId {
    PickingLayerInstanceId(u64::from(EDGE_BIT | number))
}

/// An edge's strip of `points` with the box `bounds`, added to `batch` where the view places
/// it by `stroke`, the frustum its stroke reaches the image from ([`View::place`]): not at
/// all; whole; or cut to a perspective view's pyramid ([`View::cut_strip`]). Each point,
/// or each point it's cut at, about the camera in f64 ([`View::about_camera`]), is put into
/// the view by `put`, and each strip styled by `style`. How many strips it added.
fn add_edge_strips(
    batch: &mut re_renderer::LineBatchBuilder<'_, '_>,
    view: &View,
    stroke: &ViewRegion,
    (points, bounds): (&[[f64; 3]], Bounds),
    put: impl Fn([f64; 3]) -> glam::Vec3,
    style: impl Fn(re_renderer::LineStripBuilder<'_, '_>),
) -> usize {
    match view.place(stroke, bounds, || Box::new(points.iter().copied())) {
        Placed::Not => 0,
        Placed::Whole => {
            style(batch.add_strip(points.iter().map(|p| put(view.about_camera(*p)))));
            1
        }
        Placed::Cut => {
            let strips = view.cut_strip(points, put);
            let count = strips.len();
            for strip in strips {
                style(batch.add_strip(strip.into_iter()));
            }
            count
        }
    }
}

/// Each edge of body `body`'s `uploaded` mesh of two points or more as a line strip, placed
/// in the view in f64 ([`View::about_camera`]: each point's offset from the camera's point
/// and its rotation into the view, so precise across the view at every distance and angle),
/// built for each view ([`strip_edges`], [`add_edge_strips`]): its picking ID the body's
/// number and the edge's with [`EDGE_BIT`]; a highlighted one (in `lit`) wider, in the
/// highlight's blue, in [`Highlight::Baked`], or outlined, as it is otherwise, in
/// [`Highlight::Outline`]. An edge whose box the view's pyramid holds is placed as it is;
/// one it holds none of, not at all; any other is cut to it ([`View::cut_strip`]). Each
/// point is moved toward the eye by `reach` of the view's height ([`drawn_edge_fraction`],
/// [`View::strip_from_camera`]), in f64, on the same pixel: so a concave edge's stroke is
/// drawn over the faces beside it, which rise toward the eye away from its line, as a convex
/// one's is (N5's image), and it's picked where it's drawn.
fn edges(
    lines: &mut LineDrawableBuilder<'_>,
    body: u32,
    uploaded: &Uploaded,
    lit: &Lit,
    view: &View,
    (strokes, reach): (&[ViewRegion; 2], f64),
) {
    let mut batch = edge_batch(lines, body, format!("Body{body}"));
    for (number, points, bounds) in strip_edges(uploaded) {
        let highlighted = lit.edge(number);
        let hovered = outlined_as(false, lit.hovered_edge(number));
        let (width, colour, outline) = match (highlighted, lit.style) {
            (true, Highlight::Baked) => (HIGHLIGHTED_EDGE_WIDTH, HIGHLIGHT, hovered),
            (true, Highlight::Outline) => (EDGE_WIDTH, EDGE, outlined_as(true, hovered.is_some())),
            (false, _) => (EDGE_WIDTH, EDGE, hovered),
        };
        // By the frustum its stroke reaches the image from, not its line's alone.
        let stroke = if width > EDGE_WIDTH {
            &strokes[1]
        } else {
            &strokes[0]
        };
        let style = |strip: re_renderer::LineStripBuilder<'_, '_>| {
            strip
                .radius(Size::new_ui_points(width / 2.0))
                .color(Color32::from_rgb(colour[0], colour[1], colour[2]))
                .picking_instance_id(edge_instance(number))
                .outline_mask_ids(outline);
        };
        let put = |v: [f64; 3]| view.strip_from_camera(v, reach);
        add_edge_strips(&mut batch, view, stroke, (points, bounds), put, style);
    }
}

/// A frame's `i`th set of lines as a batch of its strips, placed in the view as edges are
/// ([`add_edge_strips`]: each point about the camera in f64, [`View::about_camera`]; each
/// strip whose box the frustum its stroke reaches the image from leaves out not placed, and
/// one crossing a perspective view's pyramid cut to it), in the set's colour and width, at
/// [`EDGE_DEPTH_OFFSET`]. Each point is moved toward the eye as a drawn edge's is, by the
/// reach its own width takes ([`drawn_stroke_fraction`], [`View::strip_from_camera`]): a
/// stroke is drawn square to the view at its line's depth, so on a face not square to the
/// view (a sketch on a face, seen from aside) or along a concave edge, a face rising toward
/// the eye across it would hide half of it. Held to an edge's picking reach, as a drawn
/// edge's is, and drawn before the edges' picking strips, so those win where they tie. Its
/// picking ID is the background's, object 0 and instance 0: `re_renderer` draws every line
/// into the picking layer, and a pick passes over that ID ([`PendingPick::nearest`]), so a
/// line is picked by nothing. How many strips it drew.
fn lines_batch(
    lines: &mut LineDrawableBuilder<'_>,
    i: usize,
    set: &Lines<'_>,
    view: &View,
    pixels_per_point: f32,
) -> usize {
    let Some(bounds) = set.strips.bounds else {
        return 0;
    };
    let stroke = view.stroke_frustum(set.width, pixels_per_point);
    if stroke.class_of_box(bounds) == Class::Out {
        return 0;
    }
    let mut batch = lines
        .batch(format!("Lines{i}"))
        .world_from_obj(glam::Affine3A::IDENTITY)
        .picking_object_id(PickingLayerObjectId(0))
        .depth_offset(EDGE_DEPTH_OFFSET);
    let [red, green, blue, _] = set.colour;
    let style = |strip: re_renderer::LineStripBuilder<'_, '_>| {
        strip
            .radius(Size::new_ui_points(set.width / 2.0))
            .color(Color32::from_rgb(red, green, blue))
            .picking_instance_id(PickingLayerInstanceId(0));
    };
    let reach = drawn_stroke_fraction(view.height, set.width, pixels_per_point);
    let put = |v: [f64; 3]| view.strip_from_camera(v, reach);
    set.strips
        .strips()
        .map(|strip| add_edge_strips(&mut batch, view, &stroke, strip, put, style))
        .sum()
}

/// Body `body`'s edges of two points or more whose box lands `near` a frame's picking
/// rectangle ([`NearSides::meets_box`]), each drawn again as a strip into the picking layer
/// alone, in a batch of the body's: invisible (its colour and alpha 0, blended, so it draws
/// nothing and writes no depth to the image; outlined by nothing), [`EDGE_WIDTH`] wide at
/// [`EDGE_DEPTH_OFFSET`], its picking ID its drawn strip's (the body's number, and the edge's
/// with [`EDGE_BIT`]); found, numbered, and placed as its drawn strip is ([`strip_edges`],
/// [`add_edge_strips`]: by `stroke`, a frame's edges' frustum, whole or cut to a perspective
/// view's pyramid), then moved toward the eye by its reach ([`edge_reach`],
/// [`View::strip_from_camera`]), in f64. So an edge is picked along its stroke unless a face
/// is nearer than its reach there: a concave edge, whose faces both rise toward the eye away
/// from its line, as a convex one; a wall thicker than the reach in front of it still hides
/// it (N5). Its drawn strip is moved by no more ([`drawn_edge_fraction`]), so where both are
/// drawn this one is nearest, and the pick's depth is its. How many strips it drew.
fn edge_picks(
    lines: &mut LineDrawableBuilder<'_>,
    body: u32,
    uploaded: &Uploaded,
    view: &View,
    near: &NearSides,
    stroke: &ViewRegion,
) -> usize {
    let seen = uploaded
        .bounds
        .is_some_and(|bounds| stroke.class_of_box(bounds) != Class::Out && near.meets_box(bounds));
    if !seen {
        return 0;
    }
    let span = tracing::debug_span!("edge_picks", body, strips = tracing::field::Empty).entered();
    let mut batch = edge_batch(lines, body, format!("Body{body}EdgePicks"));
    let mut strips = 0usize;
    let reach = edge_fraction(view.height);
    for (number, points, bounds) in strip_edges(uploaded) {
        if !near.meets_box(bounds) {
            continue;
        }
        let style = |strip: re_renderer::LineStripBuilder<'_, '_>| {
            strip
                .radius(Size::new_ui_points(EDGE_WIDTH / 2.0))
                .color(Color32::TRANSPARENT)
                .picking_instance_id(edge_instance(number));
        };
        #[cfg(test)]
        EDGE_PICKS_DRAWN.with(|drawn| drawn.set(drawn.get() + 1));
        let put = |v: [f64; 3]| view.strip_from_camera(v, reach);
        strips += add_edge_strips(&mut batch, view, stroke, (points, bounds), put, style);
    }
    span.record("strips", strips);
    strips
}

/// Body `body`'s vertices of `mesh` that land `near` a picking rectangle in `view`
/// ([`Projector`], as [`project`] lands them: none nearer a perspective eye than
/// [`MIN_EYE_DISTANCE`]), each a disc `radius` points wide in the picking layer alone, in a
/// batch of the body's: invisible (its colour and alpha 0, blended, so it draws nothing and
/// writes no depth to the image), placed in the view in f64 as edges are, toward the eye by
/// [`VERTEX_REACH`] ([`View::disc_at`]) and by [`VERTEX_DEPTH_OFFSET`], its picking ID the
/// body's and [`VERTEX_BIT`] with the vertex's number. How many it drew.
fn vertices(
    points: &mut PointCloudBuilder<'_>,
    body: u32,
    mesh: &Mesh,
    view: &View,
    near: Near,
    radius: f32,
) -> usize {
    let Some(projector) = Projector::new(&view.camera, [view.width, view.height]) else {
        return 0;
    };
    let mut positions = Vec::new();
    let mut ids = Vec::new();
    for (n, p) in (1u32..).zip(&mesh.vertex_points) {
        if projector.pixel(*p).is_some_and(|pixel| near.holds(pixel)) {
            positions.push(view.disc_at(*p));
            ids.push(PickingLayerInstanceId(VERTEX_BIT | u64::from(n)));
        }
    }
    if positions.is_empty() {
        return 0;
    }
    let _span = tracing::debug_span!("vertex_discs", body, discs = positions.len()).entered();
    points
        .batch(format!("Body{body}Vertices"))
        .world_from_obj(glam::Affine3A::IDENTITY)
        .picking_object_id(PickingLayerObjectId(u64::from(body)))
        .depth_offset(VERTEX_DEPTH_OFFSET)
        .flags(
            PointCloudBatchFlags::FLAG_PREMULTIPLIED_ALPHA
                | PointCloudBatchFlags::FLAG_DRAW_AS_CIRCLES,
        )
        .add_points_slow(
            &positions,
            &[Size::new_ui_points(radius)],
            &[Color32::TRANSPARENT],
            &ids,
        );
    positions.len()
}

/// A box: its lowest and highest corners.
type Bounds = ([f64; 3], [f64; 3]);

/// How a face or an edge is drawn in a view ([`View::place`]): not at all, as it is, or cut
/// to the view's pyramid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placed {
    Not,
    Whole,
    Cut,
}

/// The box of the positions of the face `span`'s triangles; none for a face of none.
fn face_bounds(mesh: &Mesh, span: FaceSpan) -> Option<Bounds> {
    let triangles = mesh.triangles.get(span.first..span.first + span.count)?;
    box_of(
        triangles
            .iter()
            .flatten()
            .filter_map(|&v| mesh.positions.get(usize::try_from(v).ok()?)),
    )
}

/// A box's centre, halved first, so the sum can't overflow.
fn centre_of((low, high): Bounds) -> [f64; 3] {
    std::array::from_fn(|k| low[k] / 2.0 + high[k] / 2.0)
}

/// A face's vertices as uploaded: its own, renumbered from 0, their positions about its
/// anchor in f32 and their normals of unit length, and its triangles.
struct FaceVertices {
    positions: Vec<glam::Vec3>,
    normals: Vec<glam::Vec3>,
    triangles: Vec<glam::UVec3>,
}

/// The face `span` of `mesh`'s vertices about `anchor`, each position's offset from it taken
/// in f64 before it's f32.
fn face_vertices(mesh: &Mesh, span: FaceSpan, anchor: [f64; 3]) -> Result<FaceVertices, String> {
    let triangles = &mesh.triangles[span.first..span.first + span.count];
    let mut index = BTreeMap::new();
    let mut out = FaceVertices {
        positions: Vec::new(),
        normals: Vec::new(),
        triangles: Vec::with_capacity(triangles.len()),
    };
    for t in triangles {
        let mut corners = [0u32; 3];
        for (k, &v) in t.iter().enumerate() {
            let next = u32::try_from(index.len()).map_err(|e| e.to_string())?;
            let i = usize::try_from(v).map_err(|e| e.to_string())?;
            corners[k] = *index.entry(v).or_insert_with(|| {
                out.positions.push(vec3(sub(mesh.positions[i], anchor)));
                out.normals.push(unit(mesh.normals[i]));
                next
            });
        }
        out.triangles.push(glam::UVec3::from(corners));
    }
    Ok(out)
}

/// The face `span` of `mesh` cut to `view`'s pyramid, about the eye: each triangle clipped in
/// f64 in the eye's frame against the pyramid's five half-spaces (Sutherland and Hodgman's,
/// each new vertex's position and normal taken along its edge, the normal made unit length
/// again), the polygon left fanned into triangles, and each position's offset from the eye
/// f32 last. A triangle the pyramid cuts away whole leaves none.
fn cut_face_vertices(mesh: &Mesh, span: FaceSpan, view: &View) -> Result<FaceVertices, String> {
    let triangles = &mesh.triangles[span.first..span.first + span.count];
    let mut out = FaceVertices {
        positions: Vec::new(),
        normals: Vec::new(),
        triangles: Vec::new(),
    };
    if view.inside.is_none() {
        return face_vertices(mesh, span, view.origin);
    }
    for t in triangles {
        // Each corner's place in the eye's frame and its normal, in f64.
        let mut polygon: Vec<([f64; 3], [f64; 3])> = Vec::with_capacity(8);
        for &v in t {
            let i = usize::try_from(v).map_err(|e| e.to_string())?;
            let n = mesh.normals[i].map(f64::from);
            polygon.push((view.eye_frame(mesh.positions[i]), n));
        }
        for k in 0..5 {
            let mut kept = Vec::with_capacity(polygon.len() + 1);
            for (j, &(a, na)) in polygon.iter().enumerate() {
                let (b, nb) = polygon[(j + 1) % polygon.len()];
                let (fa, fb) = (view.side(k, a), view.side(k, b));
                if fa >= 0.0 {
                    kept.push((a, na));
                }
                if (fa >= 0.0) != (fb >= 0.0) {
                    let s = fa / (fa - fb);
                    let at = |p: [f64; 3], q: [f64; 3]| {
                        std::array::from_fn(|m| p[m] + (q[m] - p[m]) * s)
                    };
                    kept.push((at(a, b), at(na, nb)));
                }
            }
            polygon = kept;
            if polygon.len() < 3 {
                break;
            }
        }
        if polygon.len() < 3 {
            continue;
        }
        let base = u32::try_from(out.positions.len()).map_err(|e| e.to_string())?;
        for &(p, n) in &polygon {
            out.positions.push(vec3(view.offset_of(p)));
            let length = dot(n, n).sqrt();
            out.normals.push(if length > 0.0 {
                vec3(n.map(|c| c / length))
            } else {
                glam::Vec3::ZERO
            });
        }
        let count = u32::try_from(polygon.len()).map_err(|e| e.to_string())?;
        for j in 1..count - 1 {
            out.triangles
                .push(glam::UVec3::new(base, base + j, base + j + 1));
        }
    }
    Ok(out)
}

/// The face `span` of body `body` as a mesh of its own, of its `vertices` ([`face_vertices`],
/// or [`cut_face_vertices`]), coloured plain or baked with the highlight's blue.
fn face_cpu_mesh(
    ctx: &RenderContext,
    body: u32,
    span: FaceSpan,
    vertices: FaceVertices,
    baked: bool,
) -> Result<CpuMesh, String> {
    let FaceVertices {
        positions,
        normals,
        triangles,
    } = vertices;
    let vertices = positions.len();
    Ok(CpuMesh {
        label: format!("Body{body}Face{}", span.number).into(),
        materials: smallvec::smallvec![Material {
            label: "body".into(),
            index_range: re_renderer::Span {
                start: 0,
                len: u32::try_from(triangles.len() * 3).map_err(|e| e.to_string())?,
            },
            albedo: ctx.texture_manager_2d.white_texture_unorm_handle().clone(),
            albedo_factor: Rgba::WHITE,
        }],
        triangle_indices: triangles,
        vertex_colors: vec![vertex_colour(baked); vertices],
        vertex_normals: normals,
        vertex_texcoords: vec![glam::Vec2::ZERO; vertices],
        bbox: macaw::BoundingBox::from_points(positions.iter().copied()),
        vertex_positions: positions,
    })
}

/// The image, and the ID buffer if asked, read back once the GPU has drawn them.
fn read_back(
    ctx: &mut RenderContext,
    scene: &RenderScene,
    picking: u64,
    screenshot: u64,
) -> Result<Rendered, String> {
    let wait = || wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(GPU_WAIT),
    };
    let mut image: Option<Vec<u8>> = None;
    let mut ids: Option<Vec<PixelId>> = None;
    for _ in 0..READBACK_FRAMES {
        ctx.device
            .poll(wait())
            .map_err(|e| format!("the GPU didn't finish: {e}"))?;
        ctx.begin_frame();
        ctx.device
            .poll(wait())
            .map_err(|e| format!("the GPU didn't finish: {e}"))?;
        if image.is_none() {
            ScreenshotProcessor::next_readback_result(ctx, screenshot, |data, _, ()| {
                image = Some(data.to_vec());
            });
        }
        if scene.ids
            && ids.is_none()
            && let Some(result) = PickingLayerProcessor::readback_result(ctx, picking)
        {
            // Every ID drawn is a body's u32 and an element's u32; one that isn't would be
            // re_renderer's, shown as u32::MAX rather than wrapped onto another's.
            ids = Some(
                result
                    .picking_id_data
                    .iter()
                    .map(|id| {
                        [
                            u32::try_from(id.object.0).unwrap_or(u32::MAX),
                            u32::try_from(id.instance.0).unwrap_or(u32::MAX),
                        ]
                    })
                    .collect(),
            );
        }
        ctx.before_submit();
        if image.is_some() && (ids.is_some() || !scene.ids) {
            break;
        }
    }
    let image = image.ok_or("the image was never read back")?;
    if scene.ids && ids.is_none() {
        return Err("the ID buffer was never read back".to_owned());
    }
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    if image.len() as u64 != pixels * 4 || ids.as_ref().is_some_and(|i| i.len() as u64 != pixels) {
        return Err("a readback isn't the image's size".to_owned());
    }
    Ok(Rendered {
        png: png(&image, scene.width, scene.height)?,
        ids,
    })
}

/// RGBA pixels as an 8-bit sRGB RGB PNG, the `png` crate's default compression, no
/// metadata.
fn png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("couldn't start the PNG: {e}"))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| format!("couldn't write the PNG: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("couldn't finish the PNG: {e}"))?;
    Ok(out)
}

mod planes;

pub use planes::{MAX_FRAME_PLANES, Plane};

#[cfg(test)]
mod frame_tests;

#[cfg(test)]
mod hits_tests;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use silico_protocol::{
        Camera, Frame, InView, Mesh, Projection, RenderAdapter, RenderBody, RenderScene,
    };

    use super::{
        Bounds, EDGE_BIT, Element, MIN_EYE_DISTANCE, Near, OUTPUT_FORMAT, PixelRect, Renderer,
        Uploaded, VERTEX_BIT, View, centre_of, device, face_vertices, unit, vec3, world_from_mesh,
    };

    #[test]
    fn a_picking_instance_is_a_face_an_edge_or_a_vertex_by_its_bits_and_nothing_else() {
        let edge = u64::from(EDGE_BIT);
        for (instance, want) in [
            (1, Some(Element::Face(1))),
            (u64::from(EDGE_BIT - 1), Some(Element::Face(EDGE_BIT - 1))),
            (edge | 12, Some(Element::Edge(12))),
            (u64::from(u32::MAX), Some(Element::Edge(EDGE_BIT - 1))),
            (VERTEX_BIT | 5, Some(Element::Vertex(5))),
            // A vertex's number is the low u32 whole, the edge bit among it.
            (VERTEX_BIT | edge | 3, Some(Element::Vertex(EDGE_BIT | 3))),
            (
                VERTEX_BIT | u64::from(u32::MAX),
                Some(Element::Vertex(u32::MAX)),
            ),
            // Numbered 0: the background's, or no element.
            (0, None),
            (edge, None),
            (VERTEX_BIT, None),
            // Any other bit past a u32 is no instance drawn.
            (2 << 32 | 5, None),
            (3 << 32 | 5, None),
            (1 << 63 | 5, None),
            (u64::MAX, None),
        ] {
            assert_eq!(Element::of_instance(instance), want, "{instance:#x}");
        }
        assert_eq!(VERTEX_BIT, 0x1_0000_0000);
    }

    #[test]
    fn an_edges_reach_is_2_pixels_of_the_views_depth_at_most_a_vertexs_and_is_undone_exactly() {
        use super::{VERTEX_REACH, back_from_eye, edge_fraction, edge_reach, toward_eye};
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-12);
        // 2 pixels of the view's height: 2/800 of it at 800 rows, 2/400 at 400; at 167 rows
        // 2/167 = 0.011976, just under a vertex's 1.2%, which caps it from 166 rows (0.012048)
        // down, and for a view of none.
        assert!((edge_fraction(800) - 0.0025).abs() < 1e-18);
        assert!((edge_fraction(400) - 0.005).abs() < 1e-18);
        assert!((edge_fraction(167) - 2.0 / 167.0).abs() < 1e-18);
        for rows in [166, 48, 1, 0] {
            assert_eq!(edge_fraction(rows), VERTEX_REACH, "{rows}");
        }
        // Orthographic, 100 mm high at 800 rows: 0.25 mm, but at most half the way to the
        // camera (a point 0.4 mm in front of it moves 0.2).
        assert!((edge_reach(false, 100.0, 800, 30.0) - 0.25).abs() < 1e-12);
        assert!((edge_reach(false, 100.0, 800, 0.4) - 0.2).abs() < 1e-12);
        // In perspective, 1 mm high a mm from the eye: 2/800 of the depth, 0.25 at 100; 170°
        // high (22.86 mm a mm) at 48 rows, a vertex's 1.2% is 0.274 of the depth, past the
        // most, a quarter: 25 at 100.
        let wide = 2.0 * libm::tan(85f64.to_radians());
        assert!((edge_reach(true, 1.0, 800, 100.0) - 0.25).abs() < 1e-12);
        assert!((edge_reach(true, wide, 48, 100.0) - 25.0).abs() < 1e-12);
        // Moved and moved back, by hand: orthographic along z alone, the reach whole (10 mm
        // in front of the camera to 9.75), or half the way (0.4 to 0.2), or both alike (0.5
        // to 0.25); in perspective, along the line to the eye, by 0.9975 or the quarter.
        for (v, fraction, perspective, span, moved) in [
            ([3.0, -4.0, -10.0], 0.0025, false, 100.0, [3.0, -4.0, -9.75]),
            ([3.0, -4.0, -0.4], 0.0025, false, 100.0, [3.0, -4.0, -0.2]),
            ([3.0, -4.0, -0.5], 0.0025, false, 100.0, [3.0, -4.0, -0.25]),
            (
                [1.0, 2.0, -100.0],
                0.0025,
                true,
                1.0,
                [0.9975, 1.995, -99.75],
            ),
            (
                [1.0, 2.0, -100.0],
                VERTEX_REACH,
                true,
                wide,
                [0.75, 1.5, -75.0],
            ),
        ] {
            let there = toward_eye(v, fraction, perspective, span);
            assert!(near(there, moved), "{v:?}: {there:?}");
            let back = back_from_eye(there, fraction, perspective, span);
            assert!(near(back, v), "{v:?}: {there:?} back to {back:?}");
        }
    }

    #[test]
    fn a_drawn_edges_reach_is_root_2_strokes_and_half_a_pixel_at_most_an_edges_picking_reach() {
        use super::{VERTEX_REACH, drawn_edge_fraction, edge_fraction};
        // The stroke's half width is a point: at a pixel a point, √2 + 1/2 = 1.9142 pixels of
        // the view's height, 1.9142/800 of it at 800 rows (0.2393 mm in a view 100 mm high);
        // at half a pixel a point, √2/2 + 1/2 = 1.2071 pixels.
        let r2 = 2f64.sqrt();
        for (rows, ppp, pixels) in [
            (800, 1.0, r2 + 0.5),
            (400, 1.0, r2 + 0.5),
            (800, 0.5, r2 / 2.0 + 0.5),
            (800, 1.06, f64::from(1.06f32) * r2 + 0.5),
        ] {
            let want = pixels / f64::from(rows);
            let got = drawn_edge_fraction(rows, ppp);
            assert!(
                (got - want).abs() < 1e-15,
                "{rows} {ppp}: {got} against {want}"
            );
            assert!(got < edge_fraction(rows), "{rows} {ppp}");
        }
        // From √2 h + 1/2 = 2, h = 1.0607 pixels a point, an edge's picking reach holds it:
        // 2 pixels, 2/800 at 2 pixels a point and at the most a frame takes, 32.
        for ppp in [1.07, 2.0, 4.0, 32.0] {
            assert_eq!(drawn_edge_fraction(800, ppp), edge_fraction(800), "{ppp}");
        }
        // At 200 rows it's its own still; under 167 the picking reach is a vertex's 1.2%,
        // which holds this one too (1.9142/48 = 0.040 at 48 rows, and for a view of none).
        assert!((drawn_edge_fraction(200, 1.0) - (r2 + 0.5) / 200.0).abs() < 1e-15);
        assert_eq!(drawn_edge_fraction(48, 1.0), VERTEX_REACH);
        assert_eq!(drawn_edge_fraction(0, 1.0), VERTEX_REACH);
    }

    #[test]
    fn a_disc_reaches_a_picking_rectangle_from_its_radius_and_a_pixel_more_out() {
        // A 3-pixel square from (10, 20) to (13, 23), widened by 7 pixels and 1: x from 2 to
        // 21, y from 12 to 31, both ends taken.
        let near = Near::of(
            PixelRect {
                min: [10, 20],
                size: [3, 3],
            },
            7.0,
        );
        assert_eq!((near.low, near.high), ([2.0, 12.0], [21.0, 31.0]));
        for (pixel, holds) in [
            ([2.0, 12.0], true),
            ([21.0, 31.0], true),
            ([11.5, 21.5], true),
            ([2.0f64.next_down(), 20.0], false),
            ([10.0, 12.0f64.next_down()], false),
            ([21.0f64.next_up(), 20.0], false),
            ([10.0, 31.0f64.next_up()], false),
            ([f64::NAN, 20.0], false),
        ] {
            assert_eq!(near.holds(pixel), holds, "{pixel:?}");
        }
        // Off the frame's corner, where a rectangle may be.
        let off = Near::of(
            PixelRect {
                min: [-(1 << 24), 0],
                size: [1, 1],
            },
            0.5,
        );
        assert_eq!(off.low, [-16_777_217.5, -1.5]);
    }

    #[test]
    fn a_box_meets_a_picking_rectangle_where_its_corners_pixels_do_by_one_corner_a_side() {
        use super::Projector;
        // From (0, 0, 100) straight down, +y up, 800 by 800: orthographic 100 mm high, a point
        // (x, y) at column 400 + 8x and row 400 − 8y; in perspective 2 atan 0.5 high, 1 mm a
        // mm from the eye, at column 400 + 800 x / d and row 400 − 800 y / d, d = 100 − z its
        // depth. The rectangle the pixel (400, 400), out by 0 and a pixel more: columns and
        // rows from 399 to 402.
        let near = Near::of(
            PixelRect {
                min: [400, 400],
                size: [1, 1],
            },
            0.0,
        );
        assert_eq!((near.low, near.high), ([399.0; 2], [402.0; 2]));
        let camera = |projection| Camera {
            projection,
            eye: [0.0, 0.0, 100.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
        };
        let flat = camera(Projection::Orthographic { view_height: 100.0 });
        let deep = camera(Projection::Perspective {
            field_of_view: (2.0 * libm::atan(0.5)).to_degrees(),
        });
        let projector = |camera: &Camera| Projector::new(camera, [800, 800]).unwrap();
        // Orthographic, by hand: columns 399 to 402 are x from −0.125 to 0.25, rows 399 to
        // 402 y from 0.125 down to −0.25. A box a hundredth of a pixel inside each side meets
        // it, one a hundredth outside doesn't, at any depth.
        let hair = 0.01 / 8.0;
        let sides = near.sides(&projector(&flat));
        for (low, high, meets) in [
            ([0.25 - hair, -1.0, -1.0], [1.0, 1.0, 1.0], true),
            ([0.25 + hair, -1.0, -1.0], [1.0, 1.0, 1.0], false),
            ([-1.0, -1.0, -1.0], [-0.125 + hair, 1.0, 1.0], true),
            ([-1.0, -1.0, -1.0], [-0.125 - hair, 1.0, 1.0], false),
            ([-1.0, 0.125 - hair, -1.0], [1.0, 1.0, 1.0], true),
            ([-1.0, 0.125 + hair, -1.0], [1.0, 1.0, 1.0], false),
            ([-1.0, -1.0, -1.0], [1.0, -0.25 + hair, 1.0], true),
            ([-1.0, -1.0, -1.0], [1.0, -0.25 - hair, 1.0], false),
            ([-5.0, -5.0, -50.0], [5.0, 5.0, 50.0], true),
            ([30.0, 30.0, -1.0], [31.0, 31.0, 1.0], false),
        ] {
            assert_eq!(sides.meets_box((low, high)), meets, "{low:?} to {high:?}");
        }
        // In perspective, a box from z = −10 to 0 (depths 100 to 110) whose least x is a lands
        // from column 400 + 800 a / 110 (its far side, a above 0): column 402 at a = 0.275.
        // Reaching behind the eye (z 90 to 110, depths −10 to 10), a box from x = −50 to −40
        // lands, where it's drawn, at x/d of −4 or less, from column −2800 left: it doesn't
        // meet, though its corners behind the eye have no pixel. One across the axis does:
        // (0, 0, 95), 5 from the eye, lands on (400, 400).
        let far = 0.01 * 110.0 / 800.0;
        let sides = near.sides(&projector(&deep));
        for (low, high, meets) in [
            ([0.275 - far, -1.0, -10.0], [1.0, 1.0, 0.0], true),
            ([0.275 + far, -1.0, -10.0], [1.0, 1.0, 0.0], false),
            ([-50.0, -1.0, 90.0], [-40.0, 1.0, 110.0], false),
            ([-1.0, -1.0, 90.0], [1.0, 1.0, 110.0], true),
        ] {
            assert_eq!(sides.meets_box((low, high)), meets, "{low:?} to {high:?}");
        }
        // And over a grid of boxes wholly in front of the eye, both ways: what the box's
        // eight corners' pixels' box says, wherever that's more than a millionth of a pixel
        // from the rectangle's sides.
        for camera in [&flat, &deep] {
            let projector = projector(camera);
            let sides = near.sides(&projector);
            let (mut judged, mut met) = (0, 0);
            for (i, j, k, h) in (0..17u32)
                .flat_map(|i| (0..17u32).map(move |j| (i, j)))
                .flat_map(|(i, j)| {
                    (0..3usize).flat_map(move |k| (0..3usize).map(move |h| (i, j, k, h)))
                })
            {
                let centre = [
                    -3.0 + 0.37 * f64::from(i),
                    -3.0 + 0.37 * f64::from(j),
                    [-20.0, 0.0, 50.0][k],
                ];
                let half = [0.01, 0.3, 2.0][h];
                let bounds = (centre.map(|c| c - half), centre.map(|c| c + half));
                let pixels: Vec<[f64; 2]> = super::box_corners(bounds)
                    .into_iter()
                    .map(|corner| projector.pixel(corner).unwrap())
                    .collect();
                let least = |a: usize| pixels.iter().map(|p| p[a]).fold(f64::INFINITY, f64::min);
                let most = |a: usize| {
                    pixels
                        .iter()
                        .map(|p| p[a])
                        .fold(f64::NEG_INFINITY, f64::max)
                };
                let gaps = (0..2).flat_map(|a| [most(a) - near.low[a], near.high[a] - least(a)]);
                if gaps.clone().any(|gap| gap.abs() < 1e-6) {
                    continue;
                }
                let corners_meet = gaps.clone().all(|gap| gap > 0.0);
                assert_eq!(
                    sides.meets_box(bounds),
                    corners_meet,
                    "{:?}: {bounds:?}",
                    camera.projection
                );
                judged += 1;
                met += usize::from(corners_meet);
            }
            assert!(
                judged > 2400 && met > 50 && judged - met > 50,
                "{judged} {met}"
            );
        }
    }
    use crate::bench::{box_mesh, front_of_a_box};

    /// The view `(right, up, back)` from `(1, −1, 1)` with z up, by hand: back (1, −1, 1)/√3;
    /// up, z made perpendicular to it, (−1, 1, 2)/√6; right, up × back, (1, 1, 0)/√2.
    fn oblique() -> Frame {
        let (r2, r3, r6) = (2f64.sqrt(), 3f64.sqrt(), 6f64.sqrt());
        Frame {
            back: [1.0 / r3, -1.0 / r3, 1.0 / r3],
            up: [-1.0 / r6, 1.0 / r6, 2.0 / r6],
            right: [1.0 / r2, 1.0 / r2, 0.0],
            distance: 1.0,
        }
    }

    fn assert_near(got: glam::Vec3, want: [f64; 3], within: f64, what: &str) {
        for k in 0..3 {
            assert!(
                (f64::from(got[k]) - want[k]).abs() <= within,
                "{what}: {got:?} against {want:?}"
            );
        }
    }

    fn body(number: u32, mesh: Mesh) -> RenderBody {
        RenderBody {
            body: number,
            mesh,
            highlight_faces: vec![],
            highlight_edges: vec![],
        }
    }

    #[test]
    fn a_faces_transform_is_the_views_rotation_and_its_centre_about_the_eye() {
        // The centre (15, 5, 22) from the eye (10, 0, 20): d = (5, 5, 2). By hand, the
        // translation is (right·d, up·d, back·d) = (10/√2, 4/√6, 2/√3), and the rotation's
        // columns are the document's axes in the view: x (1/√2, −1/√6, 1/√3), y (1/√2,
        // 1/√6, −1/√3), z (0, 2/√6, 1/√3).
        let (r2, r3, r6) = (2f64.sqrt(), 3f64.sqrt(), 6f64.sqrt());
        let m = world_from_mesh(&oblique(), [10.0, 0.0, 20.0], 0.0, [15.0, 5.0, 22.0]);
        let t = glam::Vec3::from(m.translation);
        assert_near(t, [10.0 / r2, 4.0 / r6, 2.0 / r3], 1e-6, "translation");
        let columns = [
            [1.0 / r2, -1.0 / r6, 1.0 / r3],
            [1.0 / r2, 1.0 / r6, -1.0 / r3],
            [0.0, 2.0 / r6, 1.0 / r3],
        ];
        for (k, want) in columns.into_iter().enumerate() {
            assert_near(glam::Vec3::from(m.matrix3.col(k)), want, 1e-7, "column");
        }
        // A point 1 along the view's back from the centre lands 1 nearer the camera (+z):
        // the rotation takes the document to the view, not back.
        let p = m.transform_point3(vec3([1.0 / r3, -1.0 / r3, 1.0 / r3]));
        let want = [10.0 / r2, 4.0 / r6, 2.0 / r3 + 1.0];
        assert_near(p, want, 1e-6, "a point along back");
        // An orthographic view's camera sits `depth` back from its target: 30 farther along
        // −z.
        let m = world_from_mesh(&oblique(), [10.0, 0.0, 20.0], 30.0, [15.0, 5.0, 22.0]);
        let want = [10.0 / r2, 4.0 / r6, 2.0 / r3 - 30.0];
        assert_near(glam::Vec3::from(m.translation), want, 1e-5, "orthographic");
    }

    #[test]
    fn a_face_far_from_the_origin_keeps_its_offset_from_the_eye_and_its_mesh_its_shape() {
        // From the front (back −y, up z, right x), the eye 50 in front of a centre 1e9 out:
        // the offset (0.25, 50, 0.5) is exact in f64, so the translation is exactly
        // (0.25, 0.5, −50). In f32 the two points round to the same x (f32's step there is
        // 64), losing the offset: the sum isn't taken there.
        let front = Frame {
            back: [0.0, -1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            distance: 50.0,
        };
        let eye = [1e9, 2e9 - 50.0, -1e9];
        let centre_at = [1e9 + 0.25, 2e9, -1e9 + 0.5];
        let m = world_from_mesh(&front, eye, 0.0, centre_at);
        assert_eq!(
            glam::Vec3::from(m.translation),
            glam::Vec3::new(0.25, 0.5, -50.0)
        );
        assert_eq!(vec3(eye).x, vec3(centre_at).x, "f32 keeps the offset");
        // A box 0.5 wide there, as the renderer stores it: its first face (x low, at x =
        // 1e9) about its own box's centre (1e9, 2e9, −1e9 + 0.5), its corners (0, ±0.25,
        // ±0.25) in f32 exactly, in box_mesh's order; about the origin, in f32, the face's
        // four corners would be one point.
        let mesh = box_mesh([1e9, 2e9 - 0.25, -1e9 + 0.25], [0.5, 0.5, 0.5]);
        let uploaded = Uploaded::new(Arc::new(mesh.clone())).unwrap();
        let face = &uploaded.faces[0];
        assert_eq!(face.anchor, [1e9, 2e9, -1e9 + 0.5]);
        let vertices = face_vertices(&mesh, face.span, face.anchor).unwrap();
        assert_eq!(
            vertices.positions,
            [
                glam::Vec3::new(0.0, -0.25, -0.25),
                glam::Vec3::new(0.0, -0.25, 0.25),
                glam::Vec3::new(0.0, 0.25, 0.25),
                glam::Vec3::new(0.0, 0.25, -0.25),
            ]
        );
        assert_eq!(vec3(mesh.positions[0]), vec3(mesh.positions[2]));
        // The far face (x high) about its own centre, 0.5 from the first's.
        assert_eq!(uploaded.faces[1].anchor, [1e9 + 0.5, 2e9, -1e9 + 0.5]);
    }

    #[test]
    fn a_faces_box_is_its_triangles_and_an_edge_point_or_a_face_of_none_isnt_in_it() {
        // The bench's box with Face1's triangles taken by Face2 and an edge of one point a
        // kilometre off: Face1 has none, so no face; Face2's box is both x faces' (x 0 to
        // 40), Face3's the front's alone; no box holds the edge point.
        let mut mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
        mesh.face_triangles[0] = 0;
        mesh.face_triangles[1] = 4;
        mesh.edge_points.push([1e6, 0.0, 0.0]);
        mesh.edge_point_counts.push(1);
        let uploaded = Uploaded::new(Arc::new(mesh)).unwrap();
        let numbers: Vec<u32> = uploaded.faces.iter().map(|f| f.span.number).collect();
        assert_eq!(numbers, [2, 3, 4, 5, 6]);
        assert_eq!(uploaded.faces[0].bounds, ([0.0; 3], [40.0, 20.0, 10.0]));
        assert_eq!(uploaded.faces[1].bounds, ([0.0; 3], [40.0, 0.0, 10.0]));
        assert_eq!(uploaded.faces[1].anchor, [20.0, 0.0, 5.0]);
        // A face of one point at the largest coordinates is about itself; the box's corners
        // are halved before they're summed, so ±f64::MAX's centre is 0, not infinite.
        let point = Mesh {
            positions: vec![[1e12, -1e12, 3.0]; 3],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2]],
            face_triangles: vec![1],
            edge_points: vec![],
            edge_point_counts: vec![],
            vertex_points: vec![],
            valid: None,
        };
        let uploaded = Uploaded::new(Arc::new(point)).unwrap();
        assert_eq!(uploaded.faces[0].anchor, [1e12, -1e12, 3.0]);
        let wide: Bounds = ([f64::MAX, -f64::MAX, 1.0], [f64::MAX, f64::MAX, 1.0]);
        assert_eq!(centre_of(wide), [f64::MAX, 0.0, 1.0]);
    }

    #[test]
    fn an_orthographic_camera_sits_past_the_scene_and_a_perspective_one_at_the_eye() {
        // The bench's box from the front, about (21, 10, 7): its farthest corner, (0, 20, 0)
        // or (40, 20, 0) and the like, is √(21² + 10² + 7²) = √590 away by hand; the camera
        // sits that and 1 mm (a hundredth of √590 is less) back.
        let scene = front_of_a_box(64, 48);
        let view = View::of(&scene.camera, &scene).unwrap();
        assert_eq!(view.origin, scene.camera.target);
        assert!(
            (view.depth - (590f64.sqrt() + 1.0)).abs() < 1e-12,
            "{}",
            view.depth
        );
        assert_eq!(view.corner, 1.0);
        let perspective = RenderScene {
            camera: Camera {
                projection: Projection::Perspective {
                    field_of_view: 90.0,
                },
                ..scene.camera
            },
            ..scene
        };
        let view = View::of(&perspective.camera, &perspective).unwrap();
        assert_eq!((view.origin, view.depth), (perspective.camera.eye, 0.0));
        // 90°, 64 by 48: tan 45° = 1, so the corner's sec² is 1 + 1 × (1 + (4/3)²) = 34/9.
        assert!((view.corner - 34.0 / 9.0).abs() < 1e-12, "{}", view.corner);
    }

    /// `scene`'s view as it's drawn, without the scene's limits: the anchor's own rule under
    /// test, at heights and eyes the check would refuse (an orthographic view under the
    /// least its reach allows, an eye in a body's box).
    fn unchecked(scene: &RenderScene) -> View {
        let frame = scene.camera.frame().unwrap();
        let in_view = match scene.camera.projection {
            Projection::Orthographic { view_height } => InView::Orthographic {
                view_height,
                reach: scene.reach(scene.camera.target),
            },
            Projection::Perspective { field_of_view } => InView::Perspective { field_of_view },
        };
        View::new(&scene.camera, frame, [scene.width, scene.height], in_view)
    }

    /// The bench's box from the front, in perspective at `field_of_view`, `size` square, the
    /// eye at `eye` looking along +y.
    fn looking_along_y(eye: [f64; 3], field_of_view: f64, size: u32) -> RenderScene {
        RenderScene {
            camera: Camera {
                projection: Projection::Perspective { field_of_view },
                eye,
                target: [eye[0], eye[1] + 10.0, eye[2]],
                up: [0.0, 0.0, 1.0],
            },
            ..front_of_a_box(size, size)
        }
    }

    #[test]
    fn a_face_is_drawn_about_its_centre_unless_f32_about_it_would_miss_an_eighth_of_a_pixel() {
        // A face's rounding about its centre is its half diagonal h over 2^22; it's kept
        // while h 2^-22 × sec² ≤ an eighth of a pixel there. (The views, built without the
        // scene's limits: under the least height the box's reach allows, √590 × 48 / 2^19,
        // the check refuses the first.)
        //
        // Orthographic, 48 pixels high: the bench's front face (x 0 to 40, z 0 to 10 at y =
        // 0, h = √425) is about its centre from vh = h × 48 / 2^19 = 1.8874e-3 mm up. Below,
        // it's about the point on the view's line through the target (21, 10, 7) at its own
        // depth, y = 0: (21, 0, 7). The x low face (y 0 to 20, z 0 to 10 at x = 0, h = √125)
        // passes down to √125 × 48 / 2^19 = 1.02e-3: about its centre at both heights.
        let front_face: Bounds = ([0.0; 3], [40.0, 0.0, 10.0]);
        let side_face: Bounds = ([0.0; 3], [0.0, 20.0, 10.0]);
        let least = 425f64.sqrt() * 48.0 / 524_288.0;
        for (height, front_at, side_at) in [
            (least * 1.001, [20.0, 0.0, 5.0], [0.0, 10.0, 5.0]),
            (least * 0.999, [21.0, 0.0, 7.0], [0.0, 10.0, 5.0]),
        ] {
            let mut scene = front_of_a_box(64, 48);
            scene.camera.projection = Projection::Orthographic {
                view_height: height,
            };
            let view = unchecked(&scene);
            assert_eq!(view.anchor(front_face), front_at, "{height}");
            assert_eq!(view.anchor(side_face), side_at, "{height}");
        }
        // Perspective, 90°, 64 square (tan 45° = 1, the corner's sec² 1 + 1 × 2 = 3): a
        // kilometre segment (h = 500) with its nearest point d from the eye is about its
        // centre while 500 × 2^-22 × 3 ≤ d × 2 / 64 / 8, from d = 500 × 3 × 256 / 2^22 =
        // 0.091553 mm. At d = 0.06 it would pass at the image's centre (from 0.030518), but
        // not at its corner: about the eye.
        let segment: Bounds = ([-500.0, 0.0, 0.0], [500.0, 0.0, 0.0]);
        let least = 500.0 * 3.0 * 256.0 / 4_194_304.0;
        for (d, about_eye) in [(least * 1.001, false), (least * 0.999, true), (0.06, true)] {
            let scene = looking_along_y([0.0, -d, 0.0], 90.0, 64);
            let view = unchecked(&scene);
            let want = if about_eye { [0.0, -d, 0.0] } else { [0.0; 3] };
            assert_eq!(view.anchor(segment), want, "{d}");
        }
        // An eye in the face's box is about the eye, at any size; a 2 µm face a millimetre
        // beside it is about its centre.
        let scene = looking_along_y([0.0, 0.0, 0.0], 90.0, 64);
        let view = unchecked(&scene);
        let tiny: Bounds = ([-1e-3, -1e-3, -1e-3], [1e-3, 1e-3, 1e-3]);
        assert_eq!(view.anchor(tiny), [0.0; 3]);
        let tiny_off: Bounds = ([1.0, -1e-3, -1e-3], [1.002, 1e-3, 1e-3]);
        assert_eq!(view.anchor(tiny_off), [1.001, 0.0, 0.0]);
    }

    /// `test` run on lavapipe in this process: in a run of this test alone with lavapipe's
    /// variables, if this process hasn't them (a library's loader path can't be set once
    /// the process runs). `name` is the test's, in this module, or its path from the crate
    /// (`render::frame_tests::...`).
    pub(super) fn on_lavapipe(name: &str, test: impl FnOnce(&mut Renderer)) {
        if let Some(vars) = crate::lavapipe::environment()
            && vars
                .iter()
                .any(|(k, v)| std::env::var(k).map_or(true, |have| have != *v))
        {
            let name = if name.contains("::") {
                name.to_owned()
            } else {
                format!("render::tests::{name}")
            };
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([&name, "--exact", "--nocapture"])
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
        let adapters = crate::adapters(RenderAdapter::Software).unwrap();
        let (device, queue) = device(&adapters[0]).unwrap();
        test(&mut Renderer::new(&adapters[0], device, queue, OUTPUT_FORMAT).unwrap());
    }

    /// The ID buffer of `scene`.
    fn ids(renderer: &mut Renderer, scene: &RenderScene) -> Vec<[u32; 2]> {
        renderer.render(scene).unwrap().ids.unwrap()
    }

    /// The ID at the image's centre.
    fn centre_id(renderer: &mut Renderer, scene: &RenderScene) -> [u32; 2] {
        let ids = ids(renderer, scene);
        ids[usize::try_from(scene.height / 2 * scene.width + scene.width / 2).unwrap()]
    }

    /// How many face meshes `scene` uploads, and the ID at its centre.
    fn uploads_and_centre(r: &mut Renderer, scene: &RenderScene) -> (u64, [u32; 2]) {
        let before = r.uploads();
        let id = centre_id(r, scene);
        (r.uploads() - before, id)
    }

    #[test]
    fn a_new_view_of_the_same_bodies_uploads_nothing_and_draws_them_where_it_sees_them() {
        on_lavapipe(
            "a_new_view_of_the_same_bodies_uploads_nothing_and_draws_them_where_it_sees_them",
            |r| {
                // The bench's box from the front: its front face (Face3) at the centre, six
                // faces uploaded.
                let front = front_of_a_box(64, 48);
                assert_eq!(uploads_and_centre(r, &front), (6, [1, 3]));
                // From above, up +y: its top (Face6) at the centre, from the same meshes.
                let mut top = front.clone();
                top.camera = Camera {
                    eye: [21.0, 10.0, 60.0],
                    up: [0.0, 1.0, 0.0],
                    ..front.camera.clone()
                };
                assert_eq!(uploads_and_centre(r, &top), (0, [1, 6]));
                // From the right, in perspective: its right side (Face2).
                let mut right = front.clone();
                right.camera = Camera {
                    projection: Projection::Perspective {
                        field_of_view: 40.0,
                    },
                    eye: [120.0, 10.0, 5.0],
                    target: [40.0, 10.0, 5.0],
                    up: [0.0, 0.0, 1.0],
                };
                assert_eq!(uploads_and_centre(r, &right), (0, [1, 2]));
                // The front again draws the first render's ID buffer.
                let first = ids(r, &front);
                assert_eq!(ids(r, &front), first);
                assert_eq!(r.uploads(), 6);
                // A highlight uploads the face's baked mesh and drops its plain one; no view
                // after it uploads; its end uploads the plain one again.
                let mut lit = front.clone();
                lit.bodies[0].highlight_faces = vec![2];
                assert_eq!(uploads_and_centre(r, &lit), (1, [1, 3]));
                lit.camera = top.camera.clone();
                assert_eq!(uploads_and_centre(r, &lit), (0, [1, 6]));
                let face = &r.bodies[&1].faces[2];
                assert!(face.baked.is_some() && face.plain.is_none());
                assert_eq!(uploads_and_centre(r, &top), (1, [1, 6]));
                // A body whose mesh changed is uploaded again; one beside it, out of the view's
                // rectangle (100 mm off, past its 29.3 each side), isn't placed: none of it.
                let mut moved = front.clone();
                moved.bodies[0].mesh = box_mesh([1.0, 0.0, 0.0], [40.0, 20.0, 10.0]);
                moved
                    .bodies
                    .push(body(2, box_mesh([100.0, 0.0, 0.0], [1.0; 3])));
                assert_eq!(uploads_and_centre(r, &moved), (6, [1, 3]));
                // Only the last scene's bodies are kept.
                assert_eq!(r.bodies.keys().copied().collect::<Vec<_>>(), [1, 2]);
                assert_eq!(uploads_and_centre(r, &front), (6, [1, 3]));
                assert_eq!(r.bodies.keys().copied().collect::<Vec<_>>(), [1]);
                // A face highlighted the first time it's drawn is uploaded baked alone.
                let mut lit = moved.clone();
                lit.bodies.truncate(1);
                lit.bodies[0].highlight_faces = vec![2];
                assert_eq!(uploads_and_centre(r, &lit), (6, [1, 3]));
            },
        );
    }

    #[test]
    fn a_normal_is_made_unit_length_in_f64_at_any_f32_size() {
        // (0, 3, 4) is 5 long by hand: (0, 0.6, 0.8), and so at 2^100 and 2^-140 times it
        // (f32 numbers whose squares are past f32's range, or under it); f32::MAX on two
        // axes is (1/√2, 1/√2, 0); zero is none.
        let want = glam::Vec3::new(0.0, 0.6, 0.8);
        // 2^100 and 2^-140 (a subnormal) by their bits.
        for scale in [1.0f32, f32::from_bits(227 << 23), f32::from_bits(1 << 9)] {
            assert_eq!(unit([0.0, 3.0 * scale, 4.0 * scale]), want, "{scale:e}");
            assert_eq!(unit([0.0, -3.0 * scale, -4.0 * scale]), -want, "{scale:e}");
        }
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(unit([f32::MAX, f32::MAX, 0.0]), glam::Vec3::new(h, h, 0.0));
        assert_eq!(unit([0.0; 3]), glam::Vec3::ZERO);
    }

    #[test]
    fn a_render_takes_its_view_from_its_camera() {
        // The bench's box from the front (A), and from the front rolled a quarter turn, its
        // up +x (B: right is up × back = (0, 0, −1)), orthographic 44 mm over 48 rows, 64
        // wide, about (21, 10, 7): 48/44 pixels a mm. By hand, A's pixel (12, 26) is x −17.9,
        // z −2.3 about the target, (3.1, 0, 4.7) on the front face, and its pixel (34, 8) is
        // z 21.2, above the box; B's pixel (34, 8) is the front face's (35.2, 0, 4.7), and its
        // (12, 26) is z 24.9, off the box.
        on_lavapipe("a_render_takes_its_view_from_its_camera", |r| {
            let a = front_of_a_box(64, 48);
            let mut b = a.clone();
            b.camera.up = [1.0, 0.0, 0.0];
            let at = |ids: &[[u32; 2]], col: usize, row: usize| ids[row * 64 + col];
            let (seen_a, seen_b) = (ids(r, &a), ids(r, &b));
            assert_eq!(at(&seen_a, 12, 26), [1, 3]);
            assert_eq!(at(&seen_a, 34, 8), [0, 0]);
            assert_eq!(at(&seen_b, 34, 8), [1, 3]);
            assert_eq!(at(&seen_b, 12, 26), [0, 0]);
        });
    }

    #[test]
    fn placing_a_view_takes_time_within_the_render() {
        on_lavapipe("placing_a_view_takes_time_within_the_render", |r| {
            let front = front_of_a_box(64, 48);
            for _ in 0..2 {
                let started = std::time::Instant::now();
                r.render(&front).unwrap();
                let whole = started.elapsed();
                assert!(
                    r.placed() > std::time::Duration::ZERO && r.placed() <= whole,
                    "placed {:?} of {whole:?}",
                    r.placed()
                );
            }
        });
    }

    /// The `faces` of each `upload` span `f` opens, in order.
    fn uploads_spans(f: impl FnOnce()) -> Vec<u64> {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::{Context, SubscriberExt};

        #[derive(Default)]
        struct Faces(Option<u64>);
        impl tracing::field::Visit for Faces {
            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                if field.name() == "faces" {
                    self.0 = Some(value);
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        struct Spans(Arc<Mutex<Vec<u64>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Spans {
            fn on_new_span(
                &self,
                attrs: &tracing::span::Attributes<'_>,
                _: &tracing::span::Id,
                _: Context<'_, S>,
            ) {
                if attrs.metadata().name() == "upload" {
                    let mut faces = Faces::default();
                    attrs.record(&mut faces);
                    self.0.lock().unwrap().push(faces.0.unwrap_or(u64::MAX));
                }
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Spans(Arc::clone(&seen)));
        tracing::subscriber::with_default(subscriber, f);
        seen.lock().unwrap().clone()
    }

    #[test]
    fn an_upload_span_counts_the_faces_a_render_uploads_and_only_when_it_uploads_any() {
        on_lavapipe(
            "an_upload_span_counts_the_faces_a_render_uploads_and_only_when_it_uploads_any",
            |r| {
                let front = front_of_a_box(64, 48);
                let mut lit = front.clone();
                lit.bodies[0].highlight_faces = vec![2];
                let mut top = front.clone();
                top.camera.eye = [21.0, 10.0, 60.0];
                top.camera.up = [0.0, 1.0, 0.0];
                // Six faces, then none for a new view, then the one face highlighted, then it
                // plain again.
                let spans = uploads_spans(|| {
                    for scene in [&front, &top, &lit, &lit, &front] {
                        r.render(scene).unwrap();
                    }
                });
                assert_eq!(spans, [6, 1, 1]);
                assert_eq!(r.uploads(), 8);
            },
        );
    }

    #[test]
    fn a_failed_render_keeps_none_of_its_bodies() {
        on_lavapipe("a_failed_render_keeps_none_of_its_bodies", |r| {
            let front = front_of_a_box(64, 48);
            assert_eq!(uploads_and_centre(r, &front), (6, [1, 3]));
            // A degenerate camera (eye on the target) fails before anything is drawn: the
            // held bodies go with it, and the next render uploads its faces again.
            let mut broken = front.clone();
            broken.camera.eye = broken.camera.target;
            assert!(r.render(&broken).is_err());
            assert!(r.bodies.is_empty());
            assert_eq!(uploads_and_centre(r, &front), (6, [1, 3]));
        });
    }

    #[test]
    fn a_point_a_kilometre_off_a_box_on_an_edge_of_its_own_moves_none_of_its_faces() {
        // (a) A 1 mm box with an edge of one point at x = 1e6 (never drawn: an edge needs
        // two): no face's box holds it and edges have none, so a view change, close up in
        // perspective, uploads nothing, and the box's front face is at the centre.
        on_lavapipe(
            "a_point_a_kilometre_off_a_box_on_an_edge_of_its_own_moves_none_of_its_faces",
            |r| {
                let mut mesh = box_mesh([0.0; 3], [1.0; 3]);
                mesh.edge_points.push([1e6, 0.0, 0.0]);
                mesh.edge_point_counts.push(1);
                let view = |x: f64| RenderScene {
                    bodies: vec![body(1, mesh.clone())],
                    ..looking_along_y([x, -5.0, 0.5], 0.5, 64)
                };
                // Of its faces only the front and the back meet the view (0.044 mm high at the
                // front, 5 mm off at 0.5°): the others can have no pixel, and aren't placed.
                assert_eq!(uploads_and_centre(r, &view(0.5)), (2, [1, 3]));
                assert_eq!(uploads_and_centre(r, &view(0.51)), (0, [1, 3]));
                assert_eq!(uploads_and_centre(r, &view(0.49)), (0, [1, 3]));
            },
        );
    }

    #[test]
    fn a_kilometre_rod_seen_at_its_end_moves_its_four_long_faces_alone_and_draws_its_end() {
        // (b) A rod 1e6 × 20 × 10 from the origin, the eye 40 in front of it and x mm right
        // of its end, 120° across, 64 square (its far end 1e6 / 40 = 25000 times as far
        // across the view as along it, under the 2^20 tan 60° / 64 = 28378 the check takes).
        // Its long faces (h ≈ 5e5, about 40 from the eye: 12500 times, past the 2^20 tan
        // 60° / (64 × 7) = 4054 an eighth of a pixel holds at the corner, sec² 1 + 3 × 2)
        // are about the eye, uploaded again each view the eye moves. Its end caps (h = √125,
        // some 51 from the eye) stay about their centres. By hand the end, 31.8 mm left of
        // the centre, is 31.8 / 2.1651 = 14.69 pixels left of it (a pixel 2 × 40 tan 60° /
        // 64), at column 17.31: column 14 is the background and columns 21 on are the front
        // face (Face3).
        on_lavapipe(
            "a_kilometre_rod_seen_at_its_end_moves_its_four_long_faces_alone_and_draws_its_end",
            |r| {
                let rod = box_mesh([0.0; 3], [1e6, 20.0, 10.0]);
                let view = |x: f64| RenderScene {
                    bodies: vec![body(1, rod.clone())],
                    ..looking_along_y([x, -40.0, 5.0], 120.0, 64)
                };
                // Its far end, 1e6 mm off along x, is past the view's side: not placed.
                assert_eq!(uploads_and_centre(r, &view(31.8)).0, 5);
                let row = |ids: &[[u32; 2]]| ids[32 * 64..33 * 64].to_vec();
                let drawn = row(&ids(r, &view(31.8)));
                assert_eq!(drawn[14], [0, 0], "{drawn:?}");
                assert!(drawn[21..].iter().all(|id| *id == [1, 3]), "{drawn:?}");
                for x in [31.801, 31.802] {
                    assert_eq!(uploads_and_centre(r, &view(x)).0, 4, "{x}");
                }
                let moved: Vec<u32> = r.bodies[&1]
                    .faces
                    .iter()
                    .filter(|f| f.anchor == [31.802, -40.0, 5.0])
                    .map(|f| f.span.number)
                    .collect();
                assert_eq!(moved, [3, 4, 5, 6]);
            },
        );
    }

    /// A plate: Face1 a square a kilometre across its diagonals, turned to stand on a corner
    /// (its corners 500 from (0.5, 0, 0.5) along ±x and ±z, at y = 0), and Face2 a
    /// millimetre square in front of it (x and z 0 to 1, at y = −0.5).
    fn plate() -> Mesh {
        let quad = |corners: [[f64; 3]; 4]| corners;
        let faces = [
            quad([
                [-499.5, 0.0, 0.5],
                [0.5, 0.0, -499.5],
                [500.5, 0.0, 0.5],
                [0.5, 0.0, 500.5],
            ]),
            quad([
                [0.0, -0.5, 0.0],
                [1.0, -0.5, 0.0],
                [1.0, -0.5, 1.0],
                [0.0, -0.5, 1.0],
            ]),
        ];
        let mut mesh = Mesh {
            positions: vec![],
            normals: vec![],
            triangles: vec![],
            face_triangles: vec![],
            edge_points: vec![],
            edge_point_counts: vec![],
            vertex_points: vec![],
            valid: None,
        };
        for (base, corners) in (0u32..).step_by(4).zip(faces) {
            mesh.positions.extend(corners);
            mesh.normals.extend([[0.0, -1.0, 0.0]; 4]);
            mesh.triangles
                .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
            mesh.face_triangles.push(2);
        }
        mesh
    }

    #[test]
    fn an_orthographic_long_face_moves_with_the_target_only_past_an_eighth_of_a_pixel() {
        // (c) The plate from the front (−y), about (0.5, 0, 0.5), 64 pixels high: Face1's
        // box's half diagonal is 500√2, so it stays about its centre from the view height
        // h × 64 / 2^19 = 0.086316 mm up, while the plate reaches only 500 from the target,
        // so the check takes views from 500 × 64 / 2^19 = 0.061035 mm up. A pan at 1.001
        // times the first uploads nothing; at 0.999 times it, Face1 alone is uploaded again
        // about the new target. Face2 is at the centre throughout.
        on_lavapipe(
            "an_orthographic_long_face_moves_with_the_target_only_past_an_eighth_of_a_pixel",
            |r| {
                let least = 500.0 * 2f64.sqrt() * 64.0 / 524_288.0;
                assert!((least - 0.086_316).abs() < 1e-6, "{least}");
                let view = |height: f64, x: f64| RenderScene {
                    bodies: vec![body(1, plate())],
                    camera: Camera {
                        projection: Projection::Orthographic {
                            view_height: height,
                        },
                        eye: [x, -10.0, 0.5],
                        target: [x, 0.0, 0.5],
                        up: [0.0, 0.0, 1.0],
                    },
                    width: 64,
                    height: 64,
                    ids: true,
                };
                for (height, pan) in [(least * 1.001, 0), (least * 0.999, 1)] {
                    let scene = view(height, 0.5);
                    scene.check().unwrap();
                    uploads_and_centre(r, &scene);
                    for x in [0.5001, 0.5002] {
                        assert_eq!(uploads_and_centre(r, &view(height, x)), (pan, [1, 2]));
                    }
                }
            },
        );
    }

    /// A tube along z: Face1 its outside (radius `outer`), Face2 its inside (`inner`), Face3
    /// and Face4 its ends (annuli at z = 0 and `length`), `n` segments round.
    fn tube(inner: f64, outer: f64, length: f64, n: u32) -> Mesh {
        let mut mesh = Mesh {
            positions: vec![],
            normals: vec![],
            triangles: vec![],
            face_triangles: vec![],
            edge_points: vec![],
            edge_point_counts: vec![],
            vertex_points: vec![],
            valid: None,
        };
        let at = |r: f64, k: u32, z: f64| {
            let a = f64::from(k) / f64::from(n) * std::f64::consts::TAU;
            [r * libm::cos(a), r * libm::sin(a), z]
        };
        let radial = |k: u32, sign: f32| {
            let a = f64::from(k) / f64::from(n) * std::f64::consts::TAU;
            [sign * libm::cos(a) as f32, sign * libm::sin(a) as f32, 0.0]
        };
        // Each face a ring of quads between two loops of its own vertices.
        let mut ring = |loops: &dyn Fn(u32) -> ([f64; 3], [f64; 3]),
                        normal: &dyn Fn(u32) -> [f32; 3]| {
            let base = u32::try_from(mesh.positions.len()).unwrap();
            for k in 0..n {
                let (a, b) = loops(k);
                mesh.positions.extend([a, b]);
                mesh.normals.extend([normal(k); 2]);
            }
            for k in 0..n {
                let (i, j) = (base + 2 * k, base + 2 * ((k + 1) % n));
                mesh.triangles.extend([[i, j, j + 1], [i, j + 1, i + 1]]);
            }
            mesh.face_triangles.push(2 * n);
        };
        ring(&|k| (at(outer, k, 0.0), at(outer, k, length)), &|k| {
            radial(k, 1.0)
        });
        ring(&|k| (at(inner, k, 0.0), at(inner, k, length)), &|k| {
            radial(k, -1.0)
        });
        ring(&|k| (at(inner, k, 0.0), at(outer, k, 0.0)), &|_| {
            [0.0, 0.0, -1.0]
        });
        ring(&|k| (at(inner, k, length), at(outer, k, length)), &|_| {
            [0.0, 0.0, 1.0]
        });
        mesh
    }

    #[test]
    fn an_eye_inside_a_tube_cuts_its_two_walls_each_view_and_never_its_ends() {
        // A tube of radii 5 and 10, 100 long, the eye in its bore at z = 50 looking along
        // +z, 60°, 64 square. Both walls' boxes hold the eye: cut to each view's pyramid,
        // about the eye, and cut and uploaded again each view the eye moves, their normals
        // (each corner's its own, round the tube) taken along each cut and made unit length
        // again. The far end (z = 100, 50 ahead) is in the pyramid: about its centre, never
        // uploaded again. The near end (z = 0) is behind the eye: its box's corners all
        // outside the image's frustum, so it isn't placed, never cut. The centre looks
        // out through the far end; the inside wall is seen, the outside never.
        on_lavapipe(
            "an_eye_inside_a_tube_cuts_its_two_walls_each_view_and_never_its_ends",
            |r| {
                let mesh = tube(5.0, 10.0, 100.0, 48);
                let view = |x: f64, y: f64| RenderScene {
                    bodies: vec![body(1, mesh.clone())],
                    camera: Camera {
                        projection: Projection::Perspective {
                            field_of_view: 60.0,
                        },
                        eye: [x, y, 50.0],
                        target: [x, y, 100.0],
                        up: [0.0, 1.0, 0.0],
                    },
                    width: 64,
                    height: 64,
                    ids: true,
                };
                assert_eq!(uploads_and_centre(r, &view(0.0, 0.0)), (3, [0, 0]));
                for (x, y) in [(0.5, 0.0), (0.0, 0.5), (-0.5, -0.5)] {
                    assert_eq!(uploads_and_centre(r, &view(x, y)), (2, [0, 0]), "{x} {y}");
                }
                let faces = &r.bodies[&1].faces;
                assert!(faces[0].cut.is_some() && faces[1].cut.is_some());
                assert!(!faces[2].placed && faces[2].plain.is_none());
                assert!(faces[3].cut.is_none());
                let [x, y, at] = faces[3].anchor;
                assert!(
                    x.abs() < 1e-9 && y.abs() < 1e-9 && at == 100.0,
                    "{:?}",
                    faces[3].anchor
                );
                let seen = ids(r, &view(0.0, 0.0));
                assert!(seen.contains(&[1, 2]), "the inside wall isn't drawn");
                assert!(!seen.contains(&[1, 1]), "the outside wall is drawn");
                // The walls' cut vertices' normals are of unit length, across the view's
                // cuts of their round faces, and point toward the axis inside (and away out).
                let v = unchecked(&view(0.0, 0.0));
                let mesh = tube(5.0, 10.0, 100.0, 48);
                for (face, sign) in [(0usize, 1.0f32), (1, -1.0)] {
                    let span = r.bodies[&1].faces[face].span;
                    let cut = super::cut_face_vertices(&mesh, span, &v).unwrap();
                    assert!(!cut.triangles.is_empty());
                    for (p, n) in cut.positions.iter().zip(&cut.normals) {
                        assert!((n.length() - 1.0).abs() < 1e-6, "{n:?}");
                        let radial = glam::Vec2::new(p.x, p.y).normalize();
                        assert!(
                            sign * radial.dot(glam::Vec2::new(n.x, n.y)) > 0.99,
                            "{face}: {p:?} {n:?}"
                        );
                    }
                }
            },
        );
    }

    #[test]
    fn turning_or_widening_from_one_eye_cuts_the_faces_again_and_nothing_else() {
        on_lavapipe(
            "turning_or_widening_from_one_eye_cuts_the_faces_again_and_nothing_else",
            |r| {
                // From the bench's box's centre along +y at 90°, 64 by 48 (by hand, the view
                // 1 across for each mm deep up and down and 4/3 across): its back wall (y =
                // 20, 10 deep) is in the pyramid, drawn as it is; its floor and ceiling (5 off
                // the axis, in view from 5 deep) reach behind the eye's plane, cut; its x walls
                // (20 off the axis, in view only past 15 deep, and they reach 10) and its
                // front wall (behind) can have no pixel, not placed. The same view again cuts
                // nothing again; 60° from the same eye moves the pyramid's sides (B), so the
                // floor and ceiling are cut again; turning to +x moves its axes, so faces are
                // cut or placed again too.
                let at = |target: [f64; 3], field_of_view: f64| RenderScene {
                    camera: Camera {
                        projection: Projection::Perspective { field_of_view },
                        eye: [20.0, 10.0, 5.0],
                        target,
                        up: [0.0, 0.0, 1.0],
                    },
                    ..front_of_a_box(64, 48)
                };
                let before = r.uploads();
                r.render(&at([20.0, 20.0, 5.0], 90.0)).unwrap();
                assert_eq!(r.uploads() - before, 3);
                let faces = &r.bodies[&1].faces;
                for face in [0, 1, 2] {
                    assert!(!faces[face].placed, "Face{}", face + 1);
                }
                assert!(
                    faces[3].placed && faces[3].cut.is_none(),
                    "the back wall, ahead"
                );
                for face in [4, 5] {
                    assert!(
                        faces[face].placed && faces[face].cut.is_some(),
                        "Face{}",
                        face + 1
                    );
                }
                let still = r.uploads();
                r.render(&at([20.0, 20.0, 5.0], 90.0)).unwrap();
                assert_eq!(r.uploads(), still);
                r.render(&at([20.0, 20.0, 5.0], 60.0)).unwrap();
                assert_eq!(r.uploads() - still, 2);
                let widened = r.uploads();
                r.render(&at([30.0, 10.0, 5.0], 60.0)).unwrap();
                assert!(r.uploads() > widened);
            },
        );
    }

    #[test]
    fn an_eye_at_a_tubes_mouth_moves_its_two_walls_alone_and_never_its_ends() {
        // (d) A tube of radii 5 and 10, 100 long, the eye 0.005 before its mouth (z = 0)
        // looking along +z, 60°, 64 square: every point before the eye, its near end 10 /
        // 0.005 = 2000 times as far across the view as along it, under the 2^20 tan 30° / 64
        // = 9459 the pyramid holds, so none of it is cut. (From its bore, below, the walls
        // through the eye are cut to the pyramid.) Both walls' boxes (h = √(10² + 10² + 50²) = 51.96) are 0.005 from the
        // eye, 10392 times, past the 2^20 tan 30° / (64 × 5/3) = 5676 an eighth of a pixel
        // holds at the corner, sec² 1 + tan² 30° × 2: about the eye, uploaded again each
        // view the eye moves. The ends (h = 10√2) hold at 0.005 (2828 times) and at 100:
        // about their centres, never uploaded again. The view is all the bore (its edge,
        // radius 5 at 0.005, is 89.94° off the axis); the centre looks out through the far
        // end; the inside wall is seen, the outside never.
        on_lavapipe(
            "an_eye_at_a_tubes_mouth_moves_its_two_walls_alone_and_never_its_ends",
            |r| {
                let mesh = tube(5.0, 10.0, 100.0, 48);
                let at = |x: f64, y: f64, z: f64| RenderScene {
                    bodies: vec![body(1, mesh.clone())],
                    camera: Camera {
                        projection: Projection::Perspective {
                            field_of_view: 60.0,
                        },
                        eye: [x, y, z],
                        target: [x, y, 100.0],
                        up: [0.0, 1.0, 0.0],
                    },
                    width: 64,
                    height: 64,
                    ids: true,
                };
                let view = |x: f64, y: f64| at(x, y, -0.005);
                assert_eq!(uploads_and_centre(r, &view(0.0, 0.0)), (4, [0, 0]));
                for (x, y) in [(0.5, 0.0), (0.0, 0.5), (-0.5, -0.5)] {
                    assert_eq!(uploads_and_centre(r, &view(x, y)), (2, [0, 0]), "{x} {y}");
                }
                // The ends about their centres, on the axis (to the rounding of a sine).
                for (face, z) in r.bodies[&1].faces[2..].iter().zip([0.0, 100.0]) {
                    let [x, y, at] = face.anchor;
                    assert!(
                        x.abs() < 1e-9 && y.abs() < 1e-9 && at == z,
                        "{:?}",
                        face.anchor
                    );
                }
                let seen = ids(r, &view(0.0, 0.0));
                assert!(seen.contains(&[1, 2]), "the inside wall isn't drawn");
                assert!(!seen.contains(&[1, 1]), "the outside wall is drawn");
            },
        );
    }

    /// A perspective view from the origin along +y, up +z (right +x: a point (x, y, z) is
    /// (x, z) across the view at depth y), `field_of_view`, `width` by `height`, of `bodies`.
    fn from_origin(field_of_view: f64, [width, height]: [u32; 2], bodies: Vec<RenderBody>) -> View {
        unchecked(&RenderScene {
            bodies,
            camera: Camera {
                projection: Projection::Perspective { field_of_view },
                eye: [0.0; 3],
                target: [0.0, 10.0, 0.0],
                up: [0.0, 0.0, 1.0],
            },
            width,
            height,
            ids: false,
        })
    }

    #[test]
    fn an_edge_strip_through_the_eyes_plane_is_drawn_in_two_pieces_cut_at_its_nanometre() {
        // Ahead, behind, beside behind, ahead again: (−1, 5, 0), (−1, −5, 0), (1, −5, 0),
        // (1, 5, 0). By hand, B = 2^20 tan 30° / 48 = 12612.41, so a point 1 across the view
        // leaves the pyramid at depth 1 / B = 7.93e-5, before its nanometre: the first segment
        // leaves it there, the second is behind the eye, the third enters it there. Two
        // strips, each its end 5 ahead and its cut.
        let view = from_origin(60.0, [64, 48], vec![]);
        let b = view.pyramid.unwrap();
        assert!((b - 12_612.409_080_537_104).abs() < 1e-9, "{b}");
        let strips = view.cut_strip(
            &[
                [-1.0, 5.0, 0.0],
                [-1.0, -5.0, 0.0],
                [1.0, -5.0, 0.0],
                [1.0, 5.0, 0.0],
            ],
            vec3,
        );
        assert_eq!(strips.len(), 2, "{strips:?}");
        for (strip, x) in strips.iter().zip([-1.0f32, 1.0]) {
            assert_eq!(strip.len(), 2, "{strip:?}");
            let depths: Vec<f32> = strip.iter().map(|p| -p.z).collect();
            assert!(depths.contains(&5.0), "{strip:?}");
            let cut = strip.iter().find(|p| p.z != -5.0).unwrap();
            assert_eq!(cut.x, x);
            assert!((f64::from(-cut.z) * b - 1.0).abs() < 1e-6, "{cut:?}");
        }
        // Wholly behind, nothing; wholly inside, one strip of its points.
        assert!(
            view.cut_strip(&[[0.0, -1.0, 0.0], [0.0, -2.0, 0.0]], vec3)
                .is_empty()
        );
        assert_eq!(
            view.cut_strip(&[[0.0, 1.0, 0.0], [0.1, 2.0, 0.0], [0.0, 3.0, 0.1]], vec3),
            [vec![
                glam::Vec3::new(0.0, 0.0, -1.0),
                glam::Vec3::new(0.1, 0.0, -2.0),
                glam::Vec3::new(0.0, 0.1, -3.0),
            ]]
        );
    }

    /// The one face of a mesh of `triangles` (no edges), each corner's normal −y.
    fn sheet(triangles: &[[[f64; 3]; 3]]) -> Mesh {
        let positions: Vec<[f64; 3]> = triangles.iter().flatten().copied().collect();
        Mesh {
            normals: vec![[0.0, -1.0, 0.0]; positions.len()],
            triangles: (0..u32::try_from(triangles.len()).unwrap())
                .map(|t| [3 * t, 3 * t + 1, 3 * t + 2])
                .collect(),
            face_triangles: vec![u32::try_from(triangles.len()).unwrap()],
            positions,
            edge_points: vec![],
            edge_point_counts: vec![],
            vertex_points: vec![],
            valid: None,
        }
    }

    /// The cut vertices of `mesh`'s first face in `view`, in the eye's frame.
    fn cut(view: &View, mesh: &Mesh) -> (Vec<[f64; 3]>, usize) {
        let count = usize::try_from(mesh.face_triangles[0]).unwrap();
        let out = super::cut_face_vertices(
            mesh,
            super::FaceSpan {
                number: 1,
                first: 0,
                count,
            },
            view,
        )
        .unwrap();
        for n in &out.normals {
            assert!(n.is_finite() && (n.length() - 1.0).abs() < 1e-6, "{n:?}");
        }
        let points = out
            .positions
            .iter()
            .map(|p| view.eye_frame(std::array::from_fn(|k| f64::from(p[k]))))
            .collect();
        (points, out.triangles.len())
    }

    #[test]
    fn a_face_is_cut_to_the_pyramid_exactly_and_never_past_it() {
        // `/break-it` on the cut: a triangle through the eye's plane keeps only what's a
        // nanometre or more before it; one wholly behind, or beside past the pyramid, keeps
        // nothing; a degenerate one (a point, a segment) no area and no NaN; one at the
        // document's extremes stays finite; and every vertex kept is in the pyramid, to
        // f32's rounding of it.
        let view = from_origin(60.0, [64, 48], vec![]);
        let b = view.pyramid.unwrap();
        let inside = |[x, y, depth]: [f64; 3]| {
            let slack = 1e-6 * (x.abs() + y.abs() + depth.abs()).max(1e-6);
            depth >= MIN_EYE_DISTANCE - slack && x.abs() + y.abs() <= b * depth + slack
        };
        // Through the eye's plane: (0, −1, 0), (0, 1, 1), (0, 1, −1) in the document, so in
        // the eye's frame a triangle from depth −1 to 1, x = 0: cut where depth = 1e-6 and
        // where |y| = B × depth, a quadrilateral of two triangles.
        let (points, triangles) = cut(
            &view,
            &sheet(&[[[0.0, -1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 1.0, -1.0]]]),
        );
        assert!(triangles >= 1, "{points:?}");
        assert!(points.iter().all(|&p| inside(p)), "{points:?}");
        let least = points.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
        assert!((least - MIN_EYE_DISTANCE).abs() < 1e-12, "{least}");
        for case in [
            // Wholly behind.
            [[0.0, -1.0, 0.0], [1.0, -2.0, 0.0], [0.0, -1.0, 1.0]],
            // In the eye's plane, through the eye.
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            // A nanometre's hair short of it, ahead of its tip: depth 9.99e-7.
            [
                [0.0, 9.99e-7, 0.0],
                [1e-9, 9.99e-7, 0.0],
                [0.0, 9.99e-7, 1e-9],
            ],
            // Beside past the pyramid: x from 2B to 3B at depth 1.
            [
                [2.0 * b, 1.0, 0.0],
                [3.0 * b, 1.0, 0.0],
                [2.0 * b, 1.0, 1.0],
            ],
        ] {
            assert_eq!(cut(&view, &sheet(&[case])).1, 0, "{case:?}");
        }
        // Degenerate: a point, and a segment through the eye's plane, give no NaN.
        for case in [
            [[0.0, 1.0, 0.0]; 3],
            [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        ] {
            let (points, _) = cut(&view, &sheet(&[case]));
            assert!(
                points.iter().all(|p| p.iter().all(|c| c.is_finite())),
                "{points:?}"
            );
        }
        // At the document's extremes, a kilometre square 1e12 off along the view, and a
        // triangle reaching from the eye's plane to 1e12 behind and before it.
        for case in [
            [[-1e6, 1e12, -1e6], [1e6, 1e12, -1e6], [0.0, 1e12, 1e6]],
            [[0.0, -1e12, 0.0], [1e12, 1e12, 0.0], [-1e12, 1e12, 1e12]],
        ] {
            let (points, triangles) = cut(&view, &sheet(&[case]));
            assert!(triangles > 0, "{case:?}");
            assert!(
                points
                    .iter()
                    .all(|&p| p.iter().all(|c| c.is_finite()) && inside(p)),
                "{points:?}"
            );
        }
        // Slivers on the pyramid's planes: a triangle lying in its side x = B × depth, and
        // one in its floor, depth = 1e-6: kept on the plane, finite, in it.
        for case in [
            [[b, 1.0, 0.0], [2.0 * b, 2.0, 0.0], [b, 1.0, 0.0001]],
            [[0.0, 1e-6, 0.0], [1e-9, 1e-6, 0.0], [0.0, 1e-6, 1e-9]],
        ] {
            let (points, _) = cut(&view, &sheet(&[case]));
            assert!(
                points
                    .iter()
                    .all(|&p| p.iter().all(|c| c.is_finite()) && inside(p)),
                "{case:?}: {points:?}"
            );
        }
        // Random triangles about the eye, from a micrometre to a kilometre: every vertex
        // kept is finite and in the pyramid, and a triangle wholly in it is kept whole.
        let mut runner = proptest::test_runner::TestRunner::new_with_rng(
            proptest::test_runner::Config {
                cases: 3000,
                failure_persistence: None,
                ..proptest::test_runner::Config::default()
            },
            proptest::test_runner::TestRng::deterministic_rng(
                proptest::test_runner::RngAlgorithm::ChaCha,
            ),
        );
        use proptest::prelude::*;
        let scale = prop_oneof![Just(1e-6), Just(1.0), Just(1e6), 1e-6..1e6f64];
        let corner = (-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64);
        runner
            .run(
                &(scale, corner.clone(), corner.clone(), corner),
                |(k, a, b2, c)| {
                    let t = [[a.0, a.1, a.2], [b2.0, b2.1, b2.2], [c.0, c.1, c.2]]
                        .map(|p: [f64; 3]| p.map(|x| x * k));
                    let (points, triangles) = cut(&view, &sheet(&[t]));
                    for &p in &points {
                        prop_assert!(p.iter().all(|c| c.is_finite()) && inside(p), "{t:?}: {p:?}");
                    }
                    let whole = t.iter().all(|&p| {
                        let [x, z, y] = [p[0], p[2], p[1]];
                        y >= MIN_EYE_DISTANCE && x.abs() + z.abs() <= view.pyramid.unwrap() * y
                    });
                    if whole {
                        prop_assert_eq!(triangles, 1, "{:?}", t);
                    }
                    Ok(())
                },
            )
            .unwrap();
        // A vertex exactly at the pyramid's floor and on its side: in it (its half-spaces
        // are closed), kept, finite.
        let tip = [[b * 1e-6, 1e-6, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.1]];
        let (points, triangles) = cut(&view, &sheet(&[tip]));
        assert_eq!(triangles, 1, "{points:?}");
        assert!(
            points
                .iter()
                .all(|&p| p.iter().all(|c| c.is_finite()) && inside(p)),
            "{points:?}"
        );
        // A face whose box the pyramid holds isn't cut; one whose corners cross it is; one
        // wholly behind isn't placed. An orthographic view cuts nothing.
        let corners = |b: Bounds| {
            move || -> Box<dyn Iterator<Item = [f64; 3]>> {
                Box::new(silico_protocol::box_corners(b).into_iter())
            }
        };
        for (b, want) in [
            (([-1.0, 1.0, -1.0], [1.0, 2.0, 1.0]), super::Placed::Whole),
            (([-1.0, -1.0, -1.0], [1.0, 2.0, 1.0]), super::Placed::Cut),
            (([-1.0, -2.0, -1.0], [1.0, -1.0, 1.0]), super::Placed::Not),
        ] {
            assert_eq!(view.place(&view.frustum, b, corners(b)), want, "{b:?}");
        }
        let ortho = unchecked(&RenderScene {
            bodies: vec![],
            camera: Camera {
                projection: Projection::Orthographic { view_height: 4.0 },
                eye: [0.0; 3],
                target: [0.0, 10.0, 0.0],
                up: [0.0, 0.0, 1.0],
            },
            width: 64,
            height: 48,
            ids: false,
        });
        let behind = ([-1.0, -1e12, -1.0], [1.0, 2.0, 1.0]);
        assert_eq!(
            ortho.place(&ortho.frustum, behind, corners(behind)),
            super::Placed::Whole
        );
    }

    #[test]
    fn an_orbit_uploads_nothing_for_faces_in_the_pyramid_and_one_a_frame_for_a_face_it_cuts() {
        on_lavapipe(
            "an_orbit_uploads_nothing_for_faces_in_the_pyramid_and_one_a_frame_for_a_face_it_cuts",
            |r| {
                // The bench's box from 200 mm about its centre, 30°: every face in the
                // pyramid; after the first view's six, none uploaded. With a floor a kilometre
                // square under the eye (one face, in the eye's plane's way), the floor is cut
                // to each view and uploaded again for each, alone.
                let floor = sheet(&[
                    [[-5e5, -5e5, -1.0], [5e5, -5e5, -1.0], [5e5, 5e5, -1.0]],
                    [[-5e5, -5e5, -1.0], [5e5, 5e5, -1.0], [-5e5, 5e5, -1.0]],
                ]);
                let view = |k: u32, with_floor: bool| {
                    let a = f64::from(k) * 0.2;
                    let mut bodies = vec![body(1, box_mesh([0.0; 3], [40.0, 20.0, 10.0]))];
                    if with_floor {
                        bodies.push(body(2, sheet_with_normals_up(&floor)));
                    }
                    RenderScene {
                        bodies,
                        camera: Camera {
                            projection: Projection::Perspective {
                                field_of_view: 30.0,
                            },
                            eye: [
                                20.0 + 200.0 * libm::cos(a),
                                10.0 + 200.0 * libm::sin(a),
                                5.0,
                            ],
                            target: [20.0, 10.0, 5.0],
                            up: [0.0, 0.0, 1.0],
                        },
                        width: 64,
                        height: 48,
                        ids: true,
                    }
                };
                let before = r.uploads();
                for k in 0..4 {
                    r.render(&view(k, false)).unwrap();
                }
                assert_eq!(r.uploads() - before, 6);
                r.render(&view(4, true)).unwrap();
                let with_floor = r.uploads();
                for k in 5..9 {
                    r.render(&view(k, true)).unwrap();
                    assert_eq!(r.uploads() - with_floor, u64::from(k - 4), "view {k}");
                }
                // The same view again cuts nothing again.
                let again = r.uploads();
                r.render(&view(8, true)).unwrap();
                assert_eq!(r.uploads(), again);
            },
        );
    }

    /// `mesh` with every normal +z.
    fn sheet_with_normals_up(mesh: &Mesh) -> Mesh {
        Mesh {
            normals: vec![[0.0, 0.0, 1.0]; mesh.positions.len()],
            ..mesh.clone()
        }
    }

    /// A camera at `eye` looking along `direction`, up +z, `projection`.
    fn looking(eye: [f64; 3], direction: [f64; 3], projection: Projection) -> Camera {
        Camera {
            projection,
            eye,
            target: std::array::from_fn(|k| eye[k] + 10.0 * direction[k]),
            up: [0.0, 0.0, 1.0],
        }
    }

    /// `bodies` from `camera`, 64 by 48.
    fn scene_from(camera: Camera, bodies: Vec<RenderBody>) -> RenderScene {
        RenderScene {
            bodies,
            camera,
            width: 64,
            height: 48,
            ids: true,
        }
    }

    /// Each face of body 1's: whether it's placed, and whether it's cut.
    fn states(r: &Renderer) -> Vec<(bool, bool)> {
        r.bodies[&1]
            .faces
            .iter()
            .map(|f| (f.placed, f.placed && f.cut.is_some()))
            .collect()
    }

    #[test]
    fn a_room_seen_from_inside_cuts_the_walls_about_the_eye_and_places_nothing_behind() {
        on_lavapipe(
            "a_room_seen_from_inside_cuts_the_walls_about_the_eye_and_places_nothing_behind",
            |r| {
                // A metre cube from its centre, 90° over 48 rows (45° up and down, 4/3 as
                // wide: 53.1° each side), turned 10° then 22° right of +y, so no corner is
                // within a millimetre of the eye's plane. By hand: the far wall (y high) is
                // wholly ahead, drawn as it is; the floor, ceiling, and right wall reach behind
                // the eye's plane and into the view (the floor and ceiling from 500 deep), cut;
                // the left wall is wholly past the view's left side (its nearest corner 55°,
                // then 67°, off the axis) and the near wall wholly behind: not placed. (At 60°,
                // 30° up and down, the floor would come into view only past 866 deep, and the
                // room reaches 579: not placed either.) 4 meshes, then 3 for the turn (the cut ones), then 0 for a
                // still frame.
                let room = body(1, box_mesh([-500.0; 3], [1000.0; 3]));
                let at = |yaw: f64| {
                    let a = yaw.to_radians();
                    scene_from(
                        looking(
                            [0.0; 3],
                            [libm::sin(a), libm::cos(a), 0.0],
                            Projection::Perspective {
                                field_of_view: 90.0,
                            },
                        ),
                        vec![room.clone()],
                    )
                };
                let want = [
                    (false, false),
                    (true, true),
                    (false, false),
                    (true, false),
                    (true, true),
                    (true, true),
                ];
                let before = r.uploads();
                r.render(&at(10.0)).unwrap();
                assert_eq!(states(r), want);
                assert_eq!(r.uploads() - before, 4);
                let turned = r.uploads();
                r.render(&at(22.0)).unwrap();
                assert_eq!(states(r), want);
                assert_eq!(r.uploads() - turned, 3);
                let still = r.uploads();
                r.render(&at(22.0)).unwrap();
                assert_eq!(r.uploads(), still);
            },
        );
    }

    /// A quarter of a cylinder of radius 10 about the z axis, from +x to +y, 10 high: one
    /// face of `n` quads, its normals out from the axis.
    fn quarter_cylinder(n: u32) -> Mesh {
        let mut mesh = Mesh {
            positions: vec![],
            normals: vec![],
            triangles: vec![],
            face_triangles: vec![2 * n],
            edge_points: vec![],
            edge_point_counts: vec![],
            vertex_points: vec![],
            valid: None,
        };
        for k in 0..=n {
            let a = f64::from(k) / f64::from(n) * std::f64::consts::FRAC_PI_2;
            let (c, s) = (libm::cos(a), libm::sin(a));
            mesh.positions
                .extend([[10.0 * c, 10.0 * s, 0.0], [10.0 * c, 10.0 * s, 10.0]]);
            mesh.normals.extend([[c as f32, s as f32, 0.0]; 2]);
        }
        for k in 0..n {
            let (i, j) = (2 * k, 2 * k + 2);
            mesh.triangles.extend([[i, j, j + 1], [i, j + 1, i + 1]]);
        }
        mesh
    }

    #[test]
    fn a_face_whose_box_crosses_the_eyes_plane_but_whose_points_dont_is_drawn_as_it_is() {
        on_lavapipe(
            "a_face_whose_box_crosses_the_eyes_plane_but_whose_points_dont_is_drawn_as_it_is",
            |r| {
                // The quarter cylinder, the eye at (9.5, 9.5, 5) in its box's empty corner,
                // looking at the axis: the box's corner (10, 10) is behind the eye's plane,
                // but every point of the face is at least (19 − 10√2) / √2 = 3.43 before it
                // (by hand): drawn as it is, never cut, and a turn of 10° either way (every
                // point still before the eye) uploads nothing.
                let quarter = body(1, quarter_cylinder(12));
                let at = |turn: f64| {
                    let a = (225.0 + turn).to_radians();
                    scene_from(
                        looking(
                            [9.5, 9.5, 5.0],
                            [libm::cos(a), libm::sin(a), 0.0],
                            Projection::Perspective {
                                field_of_view: 60.0,
                            },
                        ),
                        vec![quarter.clone()],
                    )
                };
                let before = r.uploads();
                r.render(&at(0.0)).unwrap();
                assert_eq!(r.uploads() - before, 1);
                assert_eq!(states(r), [(true, false)]);
                let turned = r.uploads();
                for turn in [10.0, -10.0, 0.0] {
                    r.render(&at(turn)).unwrap();
                    assert_eq!(states(r), [(true, false)], "{turn}");
                }
                assert_eq!(r.uploads(), turned);
            },
        );
    }

    #[test]
    fn a_plate_behind_the_eye_whose_box_reaches_ahead_draws_nothing() {
        on_lavapipe(
            "a_plate_behind_the_eye_whose_box_reaches_ahead_draws_nothing",
            |r| {
                // A plate standing along x + y = −1, from (5, −6) to (−6, 5), 1 high, the eye
                // at the origin looking along (1, 1, 0): every point 1/√2 behind the eye's
                // plane, its box's corner (5, 5) 10/√2 ahead. Not placed by its points: no
                // pixel, no mesh.
                let plate = Mesh {
                    positions: vec![
                        [5.0, -6.0, 0.0],
                        [-6.0, 5.0, 0.0],
                        [-6.0, 5.0, 1.0],
                        [5.0, -6.0, 1.0],
                    ],
                    normals: vec![[0.0, 0.0, 1.0]; 4],
                    triangles: vec![[0, 1, 2], [0, 2, 3]],
                    face_triangles: vec![2],
                    edge_points: vec![],
                    edge_point_counts: vec![],
                    vertex_points: vec![],
                    valid: None,
                };
                let half = std::f64::consts::FRAC_1_SQRT_2;
                let scene = scene_from(
                    looking(
                        [0.0; 3],
                        [half, half, 0.0],
                        Projection::Perspective {
                            field_of_view: 60.0,
                        },
                    ),
                    vec![body(1, plate)],
                );
                let before = r.uploads();
                let seen = ids(r, &scene);
                assert!(seen.iter().all(|id| *id == [0, 0]));
                assert_eq!(r.uploads(), before);
                assert_eq!(states(r), [(false, false)]);
            },
        );
    }

    #[test]
    fn a_body_beside_the_eye_isnt_placed_and_turned_to_is_placed_with_no_upload() {
        on_lavapipe(
            "a_body_beside_the_eye_isnt_placed_and_turned_to_is_placed_with_no_upload",
            |r| {
                // A box 20 mm across, 100 mm along +x from the eye: looked at, its six faces
                // (each in the view's frustum, if hidden) uploaded; looking along +y (it's
                // some 84° off the axis, past the view's 37.6°),
                // the body isn't placed: no face, no edge, no pixel, nothing uploaded; looked
                // at again, drawn from the meshes it kept.
                let cube = body(1, box_mesh([100.0, -10.0, -10.0], [20.0; 3]));
                let at = |direction: [f64; 3]| {
                    scene_from(
                        looking(
                            [0.0; 3],
                            direction,
                            Projection::Perspective {
                                field_of_view: 60.0,
                            },
                        ),
                        vec![cube.clone()],
                    )
                };
                let before = r.uploads();
                assert_eq!(centre_id(r, &at([1.0, 0.0, 0.0])), [1, 1]);
                assert_eq!(r.uploads() - before, 6);
                let away = ids(r, &at([0.0, 1.0, 0.0]));
                assert!(away.iter().all(|id| *id == [0, 0]));
                assert!(states(r).iter().all(|(placed, _)| !placed));
                assert_eq!(r.uploads() - before, 6);
                assert_eq!(centre_id(r, &at([1.0, 0.0, 0.0])), [1, 1]);
                assert_eq!(r.uploads() - before, 6);
                // A body with no points has no box: never placed, nothing drawn.
                let empty = Mesh {
                    positions: vec![],
                    normals: vec![],
                    triangles: vec![],
                    face_triangles: vec![],
                    edge_points: vec![],
                    edge_point_counts: vec![],
                    vertex_points: vec![],
                    valid: None,
                };
                let none = ids(
                    r,
                    &scene_from(at([1.0, 0.0, 0.0]).camera, vec![body(1, empty)]),
                );
                assert!(none.iter().all(|id| *id == [0, 0]));
            },
        );
    }

    #[test]
    fn an_orthographic_view_of_part_of_a_body_places_only_the_faces_its_rectangle_meets() {
        on_lavapipe(
            "an_orthographic_view_of_part_of_a_body_places_only_the_faces_its_rectangle_meets",
            |r| {
                // The bench's box from the front, whole (its six faces uploaded), then 4 high
                // about (5, 0, 5) (by hand ±2.67 across, ±2 up): only its front and back (x 0
                // to 40, z 0 to 10, about it) meet the rectangle; its x walls (5 and 35 off),
                // its floor and top (5 off) don't. Panned to (39, 0, 5), the x high wall (1
                // off) is placed too, from the mesh it kept: nothing uploaded.
                let whole = front_of_a_box(64, 48);
                let before = r.uploads();
                r.render(&whole).unwrap();
                assert_eq!(r.uploads() - before, 6);
                let part = |x: f64| {
                    scene_from(
                        Camera {
                            projection: Projection::Orthographic { view_height: 4.0 },
                            eye: [x, -50.0, 5.0],
                            target: [x, 0.0, 5.0],
                            up: [0.0, 0.0, 1.0],
                        },
                        whole.bodies.clone(),
                    )
                };
                assert_eq!(centre_id(r, &part(5.0)), [1, 3]);
                let placed: Vec<bool> = states(r).iter().map(|s| s.0).collect();
                assert_eq!(placed, [false, false, true, true, false, false]);
                assert_eq!(centre_id(r, &part(39.0)), [1, 3]);
                let placed: Vec<bool> = states(r).iter().map(|s| s.0).collect();
                assert_eq!(placed, [false, true, true, true, false, false]);
                assert_eq!(r.uploads() - before, 6);
            },
        );
    }

    #[test]
    fn a_face_at_the_frustums_side_is_placed_and_an_ulp_past_it_isnt() {
        on_lavapipe(
            "a_face_at_the_frustums_side_is_placed_and_an_ulp_past_it_isnt",
            |r| {
                // A flat triangle 1 deep from the eye at the origin (looking +y, 60° over 48
                // rows, 64 wide), its nearest corner at x = a t × 1, on the view's right side,
                // the rest beyond it: placed; an ulp farther right, wholly beyond: not. Either
                // way it has no pixel (its one point in the view is on the image's edge).
                let tx = libm::tan(30f64.to_radians()) * (64.0 / 48.0);
                for (x, placed) in [(tx, true), (tx.next_up(), false)] {
                    let mesh = Mesh {
                        positions: vec![[x, 1.0, -0.1], [x + 1.0, 1.0, -0.1], [x, 1.0, 0.1]],
                        normals: vec![[0.0, -1.0, 0.0]; 3],
                        triangles: vec![[0, 1, 2]],
                        face_triangles: vec![1],
                        edge_points: vec![],
                        edge_point_counts: vec![],
                        vertex_points: vec![],
                        valid: None,
                    };
                    let scene = scene_from(
                        looking(
                            [0.0; 3],
                            [0.0, 1.0, 0.0],
                            Projection::Perspective {
                                field_of_view: 60.0,
                            },
                        ),
                        vec![body(1, mesh)],
                    );
                    let seen = ids(r, &scene);
                    assert!(seen.iter().all(|id| *id == [0, 0]), "{x}");
                    assert_eq!(states(r), [(placed, false)], "{x}");
                }
            },
        );
    }

    #[test]
    fn an_edge_past_the_images_side_by_less_than_its_stroke_is_drawn_to_it() {
        on_lavapipe(
            "an_edge_past_the_images_side_by_less_than_its_stroke_is_drawn_to_it",
            |r| {
                // Orthographic about the origin from −y, 48 mm over 48 rows, 64 wide: a pixel
                // is a millimetre, the image's right side at x = 32. An edge along z from −10
                // to 10 at x = 32.25 is a quarter pixel past the side, its 2-pixel stroke
                // reaching 0.75 into the image: by hand the last column (its centres 0.75 from
                // the line) is the edge's from row 16 to 32. At 32 + 3, a stroke and a pixel
                // past the side, it can't reach the image: none of it. Highlighted, its stroke
                // is 4 pixels: at 33.25 it reaches the last column too (its centres 1.75 from
                // the line), and plain it wouldn't.
                let edge_at = |x: f64, lit: bool| RenderScene {
                    bodies: vec![RenderBody {
                        body: 1,
                        mesh: Mesh {
                            positions: vec![],
                            normals: vec![],
                            triangles: vec![],
                            face_triangles: vec![],
                            edge_points: vec![[x, 0.0, -10.0], [x, 0.0, 10.0]],
                            edge_point_counts: vec![2],
                            vertex_points: vec![],
                            valid: None,
                        },
                        highlight_faces: vec![],
                        highlight_edges: if lit { vec![0] } else { vec![] },
                    }],
                    camera: Camera {
                        projection: Projection::Orthographic { view_height: 48.0 },
                        eye: [0.0, -10.0, 0.0],
                        target: [0.0; 3],
                        up: [0.0, 0.0, 1.0],
                    },
                    width: 64,
                    height: 48,
                    ids: true,
                };
                let edge = [1, silico_protocol::EDGE_BIT | 1];
                for (x, lit, drawn) in [
                    (32.25, false, true),
                    (35.0, false, false),
                    (33.25, true, true),
                    (33.25, false, false),
                ] {
                    let seen = ids(r, &edge_at(x, lit));
                    let last: Vec<[u32; 2]> = (16..=32).map(|row| seen[row * 64 + 63]).collect();
                    if drawn {
                        assert!(last.iter().all(|id| *id == edge), "{x} {lit}: {last:?}");
                    } else {
                        assert!(seen.iter().all(|id| *id == [0, 0]), "{x} {lit}");
                    }
                    // Nothing but the last column, and only the edge.
                    for (i, id) in seen.iter().enumerate() {
                        assert!(
                            *id == [0, 0] || (i % 64 == 63 && *id == edge),
                            "{x} {lit}: pixel {i}"
                        );
                    }
                }
            },
        );
    }

    #[test]
    fn an_edge_reaching_far_past_the_pyramid_or_behind_the_eye_is_drawn_where_it_projects() {
        on_lavapipe(
            "an_edge_reaching_far_past_the_pyramid_or_behind_the_eye_is_drawn_where_it_projects",
            |r| {
                // From the origin along +y, up +z, 60° over 48 rows, 64 wide: a point (x, 1,
                // z) is at column 32 + 24 x / tan 30° and row 24 − 24 z / tan 30° (by hand).
                // An edge from (−0.3, 1, −0.2), in the image, to (1e7, 1, 6e6), 1.6e7 across
                // the view at depth 1 (past the pyramid's 12,612), crosses the image to its
                // right side; another from (0.2, 1, 0.3) to (0.2, −1, 0.3), behind the eye,
                // runs from its point to the image's edge. Cut to the pyramid, every pixel of
                // each is within its stroke's pixel and an eighth of where its line projects.
                let s = 24.0 / libm::tan(30f64.to_radians());
                let lines = [
                    ([-0.3, 1.0, -0.2], [1e7, 1.0, 6e6]),
                    ([0.2, 1.0, 0.3], [0.2, -1.0, 0.3]),
                ];
                for (n, &(a, b)) in lines.iter().enumerate() {
                    let scene = RenderScene {
                        bodies: vec![body(
                            1,
                            Mesh {
                                positions: vec![],
                                normals: vec![],
                                triangles: vec![],
                                face_triangles: vec![],
                                edge_points: vec![a, b],
                                edge_point_counts: vec![2],
                                vertex_points: vec![],
                                valid: None,
                            },
                        )],
                        camera: Camera {
                            projection: Projection::Perspective {
                                field_of_view: 60.0,
                            },
                            eye: [0.0; 3],
                            target: [0.0, 10.0, 0.0],
                            up: [0.0, 0.0, 1.0],
                        },
                        width: 64,
                        height: 48,
                        ids: true,
                    };
                    let seen = ids(r, &scene);
                    // The line in the image through a's projection, along the projection's
                    // direction toward the part of b before the eye.
                    let at = |p: [f64; 3]| (32.0 + p[0] / p[1] * s, 24.0 - p[2] / p[1] * s);
                    let (x0, y0) = at(a);
                    let ahead = if b[1] > 0.0 {
                        b
                    } else {
                        // Where it meets the depth of a hundredth, still ahead.
                        let t = (a[1] - 0.01) / (a[1] - b[1]);
                        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
                    };
                    let (x1, y1) = at(ahead);
                    let (dx, dy) = (x1 - x0, y1 - y0);
                    let length = (dx * dx + dy * dy).sqrt();
                    let mut drawn = 0;
                    for (i, id) in seen.iter().enumerate() {
                        if *id == [0, 0] {
                            continue;
                        }
                        assert_eq!(*id, [1, silico_protocol::EDGE_BIT | 1], "line {n}");
                        let (px, py) = ((i % 64) as f64 + 0.5, (i / 64) as f64 + 0.5);
                        let off = ((px - x0) * dy - (py - y0) * dx).abs() / length;
                        assert!(off <= 1.125, "line {n}: pixel {i} is {off} off its line");
                        drawn += 1;
                    }
                    assert!(drawn > 30, "line {n}: {drawn} pixels");
                }
            },
        );
    }

    #[test]
    fn an_edges_stroke_frustum_is_the_images_widened_by_half_the_stroke_in_pixels() {
        // Orthographic, 48 mm over 48 rows, 64 wide (a pixel a millimetre): a stroke 2 points
        // wide at 2 pixels a point reaches 2 pixels past its line, so the frustum's sides are
        // 24 + 2 up and down and 32 + 2 left and right (by hand); 4 points at 1.5, 3 pixels.
        let view = unchecked(&RenderScene {
            bodies: vec![],
            camera: Camera {
                projection: Projection::Orthographic { view_height: 48.0 },
                eye: [0.0, -10.0, 0.0],
                target: [0.0; 3],
                up: [0.0, 0.0, 1.0],
            },
            width: 64,
            height: 48,
            ids: false,
        });
        for (width, pixels_per_point, reach) in [(2.0, 2.0, 2.0), (4.0, 1.5, 3.0)] {
            let f = view.stroke_frustum(width, pixels_per_point);
            let offsets: Vec<f64> = f.sides[1..].iter().map(|s| s.offset).collect();
            assert_eq!(
                offsets,
                [32.0 + reach, 32.0 + reach, 24.0 + reach, 24.0 + reach],
                "{width} points at {pixels_per_point}"
            );
        }
    }

    #[test]
    fn a_strip_whose_cut_rounds_to_its_end_is_ended_where_it_next_leaves_or_reenters() {
        // From the origin along +y (a point's depth its y, exactly): a segment from 1e6 deep
        // to an ulp under the pyramid's floor (1e-6) leaves it at a fraction that rounds to 1,
        // so the strip carries on to that end. Then on to 1 mm behind the eye: that segment is
        // wholly outside, and the strip [a, b] is ended there, one strip. Or back to 1 mm
        // ahead: it enters again a hair along, and the strip [a, b] is ended before another,
        // from there to c, begins: two strips.
        let view = from_origin(60.0, [64, 48], vec![]);
        let (a, b) = ([0.0, 1e6, 0.0], [0.0, 1e-6f64.next_down(), 0.0]);
        let strips = view.cut_strip(&[a, b, [0.0, -1.0, 0.0]], vec3);
        assert_eq!(strips.len(), 1, "{strips:?}");
        assert_eq!(strips[0].len(), 2, "{strips:?}");
        assert_eq!(strips[0][0], glam::Vec3::new(0.0, 0.0, -1e6));
        let strips = view.cut_strip(&[a, b, [0.0, 1.0, 0.0]], vec3);
        assert_eq!(strips.len(), 2, "{strips:?}");
        assert!(strips.iter().all(|s| s.len() == 2), "{strips:?}");
        assert_eq!(strips[0][0], glam::Vec3::new(0.0, 0.0, -1e6));
        assert_eq!(strips[1][1], glam::Vec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn a_cut_faces_normals_of_no_length_stay_of_no_length() {
        // A triangle through the eye's plane whose corners' normals are 0 (re_renderer draws
        // such a face unlit): its cut corners' normals, taken along its edges, are 0 too,
        // never 0 / 0.
        let view = from_origin(60.0, [64, 48], vec![]);
        let mut mesh = sheet(&[[[0.0, -1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 1.0, -1.0]]]);
        mesh.normals = vec![[0.0; 3]; 3];
        let cut = super::cut_face_vertices(
            &mesh,
            super::FaceSpan {
                number: 1,
                first: 0,
                count: 1,
            },
            &view,
        )
        .unwrap();
        assert!(!cut.triangles.is_empty());
        assert!(
            cut.normals.iter().all(|n| *n == glam::Vec3::ZERO),
            "{:?}",
            cut.normals
        );
    }
}
