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

use re_renderer::mesh::{CpuMesh, GpuMesh, Material};
use re_renderer::renderer::{GpuMeshInstance, MeshDrawData};
use re_renderer::view_builder::{
    OrthographicCameraMode, Projection as ViewProjection, TargetConfiguration, ViewBuilder,
    ViewBuilderId, ViewPickingConfiguration,
};
use re_renderer::{
    Color32, LineDrawableBuilder, OutlineConfig, OutlineMaskPreference, PickingLayerId,
    PickingLayerInstanceId, PickingLayerObjectId, PickingLayerProcessor, RectInt, RenderConfig,
    RenderContext, Rgba, Rgba32Unmul, ScreenshotProcessor, Size,
};
use silico_protocol::vec3::{dot, sub};
use silico_protocol::{
    Camera, EDGE_BIT, Frame as CameraFrame, InvalidParameter, MIN_VIEW_HEIGHT_OF_REACH, Mesh,
    PixelId, Projection, RenderBody, RenderScene, check_render_mesh,
};

/// The background: a flat light grey, sRGB.
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
/// A perspective view's near plane, of the scene's reach from the eye: reverse-z keeps the
/// depth precise from there out.
const NEAR_PLANE_OF_REACH: f64 = 1e-6;
/// How much a position stored in f32 about its face's centre may be off, of its distance
/// from it: f32's step (at most 2^-23 of it), twice, for the transform's own rounding.
const F32_ROUNDING: f64 = 1.0 / 4_194_304.0;
/// The most of a pixel that rounding may move a face by before it's drawn about the camera's
/// point instead of its own centre.
const PRECISION_OF_PIXEL: f64 = 0.125;
/// How far edges are drawn toward the eye, so an edge on a face isn't hidden by it.
const EDGE_DEPTH_OFFSET: re_renderer::DepthOffset = 1;
/// Frames to wait for the readbacks: re_renderer delivers them in a later frame, and each
/// frame here waits for the GPU, so they come in the first or second. The bound is for a
/// driver that never delivers them.
const READBACK_FRAMES: u32 = 100;
/// How long one wait for the GPU may take: past the 30 s the core gives a render (step 3 of
/// docs/design/m4.6-render.md), which ends a stuck worker first.
const GPU_WAIT: Duration = Duration::from_secs(60);

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
}

impl std::fmt::Display for RenderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderFailure::OutOfMemory(what) => write!(f, "out of GPU memory: {what}"),
            RenderFailure::Invalid(invalid) => f.write_str(&invalid.message),
            RenderFailure::Failed(what) => f.write_str(what),
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

/// A body's face or edge, by its number from 1, as the ID buffer and the elements' names
/// number them (`Face3`, `Edge12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Element {
    Face(u32),
    Edge(u32),
}

impl Element {
    /// The element a picking ID's instance is: a face's number from 1, or an edge's with
    /// [`EDGE_BIT`]; none for anything else (0, an edge numbered 0, or past a u32), which
    /// no instance drawn has.
    fn of_instance(instance: u64) -> Option<Element> {
        let n = u32::try_from(instance).ok()?;
        if n & EDGE_BIT != 0 {
            let edge = n & !EDGE_BIT;
            (edge > 0).then_some(Element::Edge(edge))
        } else {
            (n > 0).then_some(Element::Face(n))
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
    /// picking ID.
    pub highlights: &'a [(u32, Element)],
    /// The pixels to read back for picking, if any: [`Renderer::take_picking`] gives what
    /// they found, a frame or two later.
    pub picking: Option<PixelRect>,
}

/// What a picking rectangle found nearest its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    /// The body's number.
    pub body: u32,
    /// Its face or edge there.
    pub hit: Element,
    /// The pixel it was found at, in the frame.
    pub pixel: [u32; 2],
    /// The point seen at the pixel's centre, in the document's coordinates: the centre
    /// unprojected at the picking layer's depth there, in f64 (the depth itself is the GPU's
    /// f32); none if the depth wasn't read back.
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
        let ctx = RenderContext::new(
            adapter,
            device,
            queue,
            output_format,
            RenderConfig::best_for_device_caps,
        )
        .map_err(|e| format!("no render context: {e}"))?;
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
        })
    }

    /// How many face meshes it has uploaded since it was made: a face's, plain or in the
    /// highlight's tint, the first time it's drawn so, and again after its body's mesh, the
    /// point it's drawn about ([`View::anchor`]), or its highlight changed.
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    /// How long the last render or frame took to place its faces and edges for its view on
    /// the CPU (each face's anchor and transform, its instance, and the edges' strips),
    /// uploads included: for the bench, never a result.
    pub fn placed(&self) -> Duration {
        self.placed
    }

    /// `mesh` as body `body`'s (a number from 1), for frames to draw: kept until it's
    /// forgotten or replaced, and its faces uploaded to the GPU the first time a frame
    /// draws them, never again for another view (but a large face seen close,
    /// [`View::anchor`]). The same mesh again (the same `Arc`, or an equal mesh) changes
    /// nothing; another replaces the body's, so its faces are uploaded again. A mesh that
    /// fails [`check_render_mesh`], or a body numbered 0 (the background's), is refused,
    /// and the body is as it was.
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
    /// highlighted face's and edge's instance is outlined in the highlight's blue, its mesh
    /// and colour as uploaded, so a highlight uploads nothing ([`Highlight::Outline`]); the picking
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
        let (view, lit) = self.check_frame(frame).map_err(RenderFailure::Invalid)?;
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
                },
            );
            (rect_int(rect), id)
        });
        let plain = Lit::default();
        let drawn: Vec<(u32, &Lit)> = frame
            .bodies
            .iter()
            .map(|&b| (b, lit.get(&b).unwrap_or(&plain)))
            .collect();
        let built = self.build(&view, frame.pixels_per_point, &drawn, picking, None);
        self.ctx.before_submit();
        // On wgpu's native backends these are answered at once: no wait.
        let invalid = pollster::block_on(validity.pop());
        let short = pollster::block_on(memory.pop());
        match scoped(short, invalid, built) {
            Ok((commands, builder)) => {
                self.view = Some(builder);
                Ok(commands)
            }
            Err(failure) => {
                if let Some((_, id)) = picking {
                    self.pending.remove(&id);
                }
                // Nothing vouches for buffers a failed frame made: none is drawn again.
                for body in self.bodies.values_mut() {
                    body.drop_gpu_meshes();
                }
                Err(failure)
            }
        }
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
        let hit = request.nearest(&ids, extent, current).map(|(i, mut pick)| {
            if depths.len() == ids.len()
                && let Some(&depth) = usize::try_from(i).ok().and_then(|i| depths.get(i))
            {
                pick.point = request.point_at(pick.pixel, depth);
            }
            pick
        });
        Some(Picking {
            frame: request.serial,
            rect: request.rect,
            hit,
        })
    }

    /// `frame`'s checks, and what it draws: its view, and each body's highlights.
    fn check_frame(
        &self,
        frame: &Frame<'_>,
    ) -> Result<(View, BTreeMap<u32, Lit>), InvalidParameter> {
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
        let ppp = frame.pixels_per_point;
        if !(ppp.is_finite() && ppp > 0.0 && ppp <= MAX_PIXELS_PER_POINT) {
            return Err(InvalidParameter {
                parameter: "pixels_per_point",
                message: format!(
                    "pixels_per_point {ppp:?} must be above 0 and at most {MAX_PIXELS_PER_POINT}"
                ),
            });
        }
        let mut last = 0;
        for (i, &body) in frame.bodies.iter().enumerate() {
            if body <= last {
                return Err(InvalidParameter {
                    parameter: "bodies",
                    message: format!(
                        "bodies[{i}], {body}, must be above the one before it ({last}): ascending, each once, from 1"
                    ),
                });
            }
            if !self.bodies.contains_key(&body) {
                return Err(InvalidParameter {
                    parameter: "bodies",
                    message: format!("bodies[{i}], {body}, isn't uploaded"),
                });
            }
            last = body;
        }
        let mut lit: BTreeMap<u32, Lit> = BTreeMap::new();
        for (i, &(body, element)) in frame.highlights.iter().enumerate() {
            let Some(uploaded) = frame
                .bodies
                .binary_search(&body)
                .ok()
                .and_then(|_| self.bodies.get(&body))
            else {
                return Err(InvalidParameter {
                    parameter: "highlights",
                    message: format!("highlights[{i}]'s body {body} isn't drawn in the frame"),
                });
            };
            let mesh = &uploaded.mesh;
            let (number, count, what) = match element {
                Element::Face(n) => (n, mesh.face_triangles.len(), "faces"),
                Element::Edge(n) => (n, mesh.edge_point_counts.len(), "edges"),
            };
            if !usize::try_from(number).is_ok_and(|n| (1..=count).contains(&n)) {
                return Err(InvalidParameter {
                    parameter: "highlights",
                    message: format!(
                        "highlights[{i}], {element:?} of body {body}, isn't one of its {count} {what} (numbered from 1)"
                    ),
                });
            }
            let entry = lit.entry(body).or_default();
            match element {
                Element::Face(n) => entry.faces.push(n),
                Element::Edge(n) => entry.edges.push(n),
            }
        }
        for entry in lit.values_mut() {
            for numbers in [&mut entry.faces, &mut entry.edges] {
                numbers.sort_unstable();
                numbers.dedup();
            }
        }
        if let Some(rect) = frame.picking {
            let sides = rect.size.iter().all(|s| (1..=MAX_PICKING_SIDE).contains(s));
            let corner = rect
                .min
                .iter()
                .all(|c| c.unsigned_abs() <= MAX_PICKING_OFFSET.unsigned_abs());
            if !(sides && corner) {
                return Err(InvalidParameter {
                    parameter: "picking",
                    message: format!(
                        "picking {rect:?} must be from 1 to {MAX_PICKING_SIDE} pixels on each side, its corner within {MAX_PICKING_OFFSET} of the frame's"
                    ),
                });
            }
        }
        // The bodies' reach, from their boxes' farthest corners: at most √3 past the
        // farthest point, which only sets the clipping, past everything drawn either way.
        let boxes: Vec<Bounds> = frame
            .bodies
            .iter()
            .filter_map(|b| self.bodies.get(b).and_then(|u| u.bounds))
            .collect();
        let reach = |from: [f64; 3]| {
            boxes
                .iter()
                .map(|&bounds| farthest(bounds, from))
                .fold(0.0, f64::max)
        };
        let view = View::new(frame.camera, frame.size_px, &reach)?;
        if let Projection::Orthographic { view_height } = frame.camera.projection {
            let reach = reach(frame.camera.target);
            let least = reach * MIN_VIEW_HEIGHT_OF_REACH;
            if view_height < least {
                return Err(InvalidParameter {
                    parameter: "camera.view_height",
                    message: format!(
                        "camera.view_height {view_height:?} is too small to draw the bodies in view, which reach {reach:?} mm from camera.target: at least {least:?}, or a camera.target nearer them"
                    ),
                });
            }
        }
        Ok((view, lit))
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
            if let Some(request) = self.pending.remove(&id) {
                self.picked = Some(Answer {
                    request,
                    ids: result.picking_id_data,
                    depths: result.picking_depth_data,
                    extent: result.rect.extent,
                });
            }
            self.pending.retain(|&k, _| k > id);
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
        for body in &scene.bodies {
            if let std::collections::btree_map::Entry::Vacant(entry) = self.bodies.entry(body.body)
            {
                let mut uploaded = Uploaded::new(Arc::new(body.mesh.clone()))?;
                self.generations += 1;
                uploaded.generation = self.generations;
                entry.insert(uploaded);
            }
        }
        let lit: Vec<Lit> = scene.bodies.iter().map(Lit::of_body).collect();
        let drawn: Vec<(u32, &Lit)> = scene.bodies.iter().map(|b| b.body).zip(&lit).collect();
        let whole = RectInt {
            min: glam::IVec2::ZERO,
            extent: glam::UVec2::new(scene.width, scene.height),
        };
        self.ctx.begin_frame();
        let built = self.build(
            &view,
            1.0,
            &drawn,
            scene.ids.then_some((whole, picking)),
            Some(screenshot),
        );
        self.ctx.before_submit();
        let (commands, _view) = built?;
        self.ctx.queue.submit([commands]);
        read_back(&mut self.ctx, scene, picking, screenshot)
    }

    /// The view `view` of the bodies `drawn` (each uploaded, with its highlights), in a
    /// frame `re_renderer`'s already begun: each face an instance moved into the view, its
    /// mesh uploaded where it has none; each edge a strip placed in the view; highlights
    /// drawn in each body's [`Lit::style`]; the picking rectangle and its readback's identifier, and
    /// the image's, if asked. The command buffer that draws it into the view's target, and
    /// the view, to composite.
    fn build(
        &mut self,
        view: &View,
        pixels_per_point: f32,
        drawn: &[(u32, &Lit)],
        picking: Option<(RectInt, u64)>,
        screenshot: Option<u64>,
    ) -> Result<(wgpu::CommandBuffer, ViewBuilder), String> {
        let Renderer {
            ctx,
            bodies,
            uploads,
            placed,
            ..
        } = self;
        let placing = std::time::Instant::now();
        let mut instances = Vec::new();
        let mut lines = LineDrawableBuilder::new(ctx);
        lines.enable_alpha_blending();
        for &(number, lit) in drawn {
            let uploaded = bodies
                .get_mut(&number)
                .ok_or_else(|| format!("body {number} isn't uploaded"))?;
            uploaded.settle(view, lit);
            uploaded.faces(ctx, number, lit, view, &mut instances, uploads)?;
            edges(&mut lines, number, &uploaded.mesh, lit, view);
        }
        *placed = placing.elapsed();
        let highlighted = drawn
            .iter()
            .any(|(_, lit)| !lit.faces.is_empty() || !lit.edges.is_empty());
        let config = TargetConfiguration {
            name: "render".into(),
            resolution_in_pixel: [view.width, view.height],
            // The view is the default, the identity: the camera at the world's origin, looking
            // along −z with +y up. The camera reaches the GPU through each face's
            // `world_from_mesh` and the edges' placement, never through the view
            // (`a_render_takes_its_view_from_its_camera`).
            projection_from_view: view.projection,
            pixels_per_point,
            outline_config: highlighted.then(|| OutlineConfig {
                outline_radius_pixel: OUTLINE_WIDTH * pixels_per_point,
                color_layer_a: srgb(HIGHLIGHT),
                color_layer_b: Rgba::TRANSPARENT,
            }),
            picking_config: picking.map(|(picking_rect, readback_identifier)| {
                ViewPickingConfiguration {
                    picking_rect,
                    readback_identifier,
                    show_debug_view: false,
                }
            }),
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
        if let Some(screenshot) = screenshot {
            builder
                .schedule_screenshot(ctx, screenshot, ())
                .map_err(|e| format!("couldn't schedule the image's readback: {e}"))?;
        }
        let commands = builder
            .draw(ctx, srgb(BACKGROUND))
            .map_err(|e| format!("couldn't draw: {e}"))?;
        Ok((commands, builder))
    }
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

/// How far a box's farthest corner is from `from`.
fn farthest((low, high): Bounds, from: [f64; 3]) -> f64 {
    let d: [f64; 3] =
        std::array::from_fn(|k| (from[k] - low[k]).abs().max((high[k] - from[k]).abs()));
    dot(d, d).sqrt()
}

/// How highlights are drawn. The worker's renders tint a highlighted face (its mesh
/// uploaded in the tint) and outline it, and draw a highlighted edge wider and blue: the
/// agents' PNG look (docs/design/m4.6-render.md). The app's frames outline each highlighted
/// face's and edge's instance alone, with `re_renderer`'s per-instance outline masks, as
/// Rerun's viewer draws hover and selection, and leave its mesh and colour as uploaded: so a
/// highlight, which moves with the pointer, never uploads anything, and changes no pixel of
/// the picking layer (docs/design/m5.0-app.md section 10, "outlines by instance").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Highlight {
    Tint,
    /// A frame's: and a body's with no highlights, which looks the same either way.
    #[default]
    Outline,
}

/// The outline mask a highlighted instance is drawn with: the outline config's first layer.
fn outlined() -> OutlineMaskPreference {
    OutlineMaskPreference::some(1, 0)
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
}

impl PendingPick {
    /// Of the readback's pixels `ids`, `extent` in size: of the frame's own (outside the
    /// frame, re_renderer draws on as if it were larger, which isn't shown), the one with a
    /// body nearest the rectangle's centre, the first in rows of those as near, of the
    /// bodies `current` takes (those not, the background's); its index in `ids`, and
    /// what's there, without its point.
    fn nearest(
        &self,
        ids: &[PickingLayerId],
        extent: glam::UVec2,
        current: impl Fn(u32) -> bool,
    ) -> Option<(u32, Pick)> {
        let width = extent.x;
        let pixels = u64::from(width) * u64::from(extent.y);
        if ids.len() as u64 != pixels || width == 0 {
            return None;
        }
        let centre = self.rect.size.map(|s| f64::from(s) / 2.0);
        let mut nearest: Option<(f64, u32, Pick)> = None;
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
            if nearest.is_none_or(|(best, ..)| d2 < best) {
                let pick = Pick {
                    body,
                    hit,
                    pixel: [px, py],
                    point: None,
                };
                nearest = Some((d2, i, pick));
            }
        }
        nearest.map(|(_, i, pick)| (i, pick))
    }

    /// The point seen at the centre of the frame's pixel `pixel`, at the picking layer's
    /// `depth` there, in the document: the pixel's centre and the depth unprojected through
    /// the frame's own projection (the picking rectangle's crop moves and scales x and y
    /// alone, so its depth is the frame's), in f64, then taken back through the view (the
    /// inverse of [`View::place`], d = right x + up y + back (z + depth)); none if it isn't
    /// finite.
    fn point_at(&self, pixel: [u32; 2], depth: f32) -> Option<[f64; 3]> {
        let [width, height] = self.size.map(f64::from);
        let ndc = glam::DVec3::new(
            (f64::from(pixel[0]) + 0.5) / width * 2.0 - 1.0,
            1.0 - (f64::from(pixel[1]) + 0.5) / height * 2.0,
            f64::from(depth),
        );
        let view_from_projection = self
            .projection
            .projection_from_view(self.size)
            .as_dmat4()
            .inverse();
        let p = view_from_projection.project_point3(ndc);
        if !p.is_finite() {
            return None;
        }
        let v = [p.x, p.y, p.z + self.depth];
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
}

impl View {
    /// `scene`'s view, from its camera and its bodies' reach.
    fn of(camera: &Camera, scene: &RenderScene) -> Result<View, String> {
        View::new(camera, [scene.width, scene.height], &|p| scene.reach(p)).map_err(|e| e.message)
    }

    /// The view from `camera` of an image `width` by `height`, of bodies that reach `reach`
    /// from a point.
    fn new(
        camera: &Camera,
        [width, height]: [u32; 2],
        reach: &dyn Fn([f64; 3]) -> f64,
    ) -> Result<View, InvalidParameter> {
        let frame = camera.frame()?;
        let view = match camera.projection {
            Projection::Orthographic { view_height } => {
                // Only the eye's direction counts: the camera is set past every point of the
                // scene, so nothing is cut off behind the eye (FreeCAD fits its clipping
                // planes to the scene, too), and the far plane past the scene's other side.
                let reach = reach(camera.target);
                let margin = (reach * CLEARANCE).max(1.0);
                View {
                    frame,
                    origin: camera.target,
                    depth: reach + margin,
                    span: view_height,
                    perspective: false,
                    width,
                    height,
                    corner: 1.0,
                    projection: ViewProjection::Orthographic {
                        camera_mode: OrthographicCameraMode::NearPlaneCenter,
                        vertical_world_size: view_height as f32,
                        far_plane_distance: (2.0 * (reach + margin)) as f32,
                    },
                }
            }
            Projection::Perspective { field_of_view } => {
                // The near plane from what's in view, not from where the target is along
                // the ray: a millionth of the farthest point's distance (reverse-z keeps the
                // depth precise from there out).
                let reach = reach(camera.eye).max(frame.distance);
                let tan = libm::tan(field_of_view.to_radians() / 2.0);
                let aspect = f64::from(width) / f64::from(height);
                View {
                    frame,
                    origin: camera.eye,
                    depth: 0.0,
                    span: 2.0 * tan,
                    perspective: true,
                    width,
                    height,
                    corner: 1.0 + tan * tan * (1.0 + aspect * aspect),
                    projection: ViewProjection::Perspective {
                        vertical_fov: field_of_view.to_radians() as f32,
                        near_plane_distance: (reach * NEAR_PLANE_OF_REACH) as f32,
                        aspect_ratio: width as f32 / height as f32,
                    },
                }
            }
        };
        Ok(view)
    }
    fn world_from_mesh(&self, anchor: [f64; 3]) -> glam::Affine3A {
        world_from_mesh(&self.frame, self.origin, self.depth, anchor)
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

    /// `p` in the fixed camera's world, its offset from the camera's point and its rotation
    /// into the view both taken in f64 before it's f32: precise to f32's step of what's
    /// across the view, wherever the point is.
    fn place(&self, p: [f64; 3]) -> glam::Vec3 {
        let d = sub(p, self.origin);
        vec3([
            dot(self.frame.right, d),
            dot(self.frame.up, d),
            dot(self.frame.back, d) - self.depth,
        ])
    }
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

/// A body as uploaded: what it was made from, to know it again and draw its edges from, its
/// box (its positions' and edge points', none for a mesh of neither), and its faces.
struct Uploaded {
    mesh: Arc<Mesh>,
    /// Which of the renderer's meshes it is ([`Renderer::next_generation`]); 0 until the
    /// renderer gives it one.
    generation: u64,
    bounds: Option<Bounds>,
    faces: Vec<UploadedFace>,
}

/// What of a body is highlighted: its faces' and edges' numbers from 1, ascending, each
/// once; and how.
#[derive(Default)]
struct Lit {
    faces: Vec<u32>,
    edges: Vec<u32>,
    style: Highlight,
}

impl Lit {
    /// A scene's body's highlights, which it numbers from 0.
    fn of_body(body: &RenderBody) -> Lit {
        let numbers = |from_0: &[u32]| from_0.iter().map(|i| i.saturating_add(1)).collect();
        Lit {
            faces: numbers(&body.highlight_faces),
            edges: numbers(&body.highlight_edges),
            style: Highlight::Tint,
        }
    }

    fn face(&self, number: u32) -> bool {
        self.faces.binary_search(&number).is_ok()
    }

    fn edge(&self, number: u32) -> bool {
        self.edges.binary_search(&number).is_ok()
    }
}

/// A face with triangles: which it is, its box (its triangles' positions'), the point its
/// mesh is about ([`View::anchor`]), and its mesh, plain or tinted, as the last render drew
/// it, each uploaded the first time a render draws the face so about that point.
struct UploadedFace {
    span: FaceSpan,
    bounds: Bounds,
    anchor: [f64; 3],
    plain: Option<Arc<GpuMesh>>,
    tinted: Option<Arc<GpuMesh>>,
}

/// A face's number from 1, and its `count` triangles' place, from `first`, in its body's
/// mesh.
#[derive(Clone, Copy)]
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
                plain: None,
                tinted: None,
            });
        }
        Ok(Uploaded {
            generation: 0,
            bounds: bounds_of(mesh.positions.iter().chain(&mesh.edge_points)),
            mesh,
            faces,
        })
    }

    /// Its faces' GPU meshes dropped, to be uploaded again when they're next drawn.
    fn drop_gpu_meshes(&mut self) {
        for face in &mut self.faces {
            face.plain = None;
            face.tinted = None;
        }
    }

    /// Each face's point for `view`, and what it won't draw dropped: its meshes about
    /// another point, and the colour it isn't drawn in (tinted where `lit` highlights it
    /// in [`Highlight::Tint`], plain otherwise).
    fn settle(&mut self, view: &View, lit: &Lit) {
        for face in &mut self.faces {
            let anchor = view.anchor(face.bounds);
            if anchor != face.anchor {
                face.anchor = anchor;
                face.plain = None;
                face.tinted = None;
            }
            if lit.style == Highlight::Tint && lit.face(face.span.number) {
                face.plain = None;
            } else {
                face.tinted = None;
            }
        }
    }

    /// Each face as a mesh instance of its own, moved into `view` by its own transform, its
    /// picking ID the body's number `body` and the face's; a highlighted one (in `lit`)
    /// outlined, and tinted in [`Highlight::Tint`]. A face's mesh is uploaded where it has
    /// none (counted in `uploads`).
    fn faces(
        &mut self,
        ctx: &RenderContext,
        body: u32,
        lit: &Lit,
        view: &View,
        instances: &mut Vec<GpuMeshInstance>,
        uploads: &mut u64,
    ) -> Result<(), String> {
        // The faces to be uploaded: each without the mesh, plain or tinted, it's drawn with.
        let new = self
            .faces
            .iter()
            .filter(|f| f.plain.is_none() && f.tinted.is_none())
            .count();
        let _span = (new > 0).then(|| tracing::debug_span!("upload", body, faces = new).entered());
        for face in &mut self.faces {
            let number = face.span.number;
            let highlighted = lit.face(number);
            let tinted = lit.style == Highlight::Tint && highlighted;
            let held = if tinted {
                &mut face.tinted
            } else {
                &mut face.plain
            };
            let gpu_mesh = match held {
                Some(mesh) => Arc::clone(mesh),
                None => {
                    let cpu = face_cpu_mesh(ctx, body, &self.mesh, face.anchor, face.span, tinted)?;
                    let gpu = GpuMesh::new(ctx, &cpu).map_err(|e| format!("Face{number}: {e}"))?;
                    *uploads += 1;
                    Arc::clone(held.insert(Arc::new(gpu)))
                }
            };
            instances.push(GpuMeshInstance {
                gpu_mesh,
                world_from_mesh: view.world_from_mesh(face.anchor),
                // Opaque black: re_renderer adds the tint's colour and multiplies by its
                // alpha, so this leaves the face's own colour.
                additive_tint: Color32::BLACK,
                outline_mask_ids: if highlighted {
                    outlined()
                } else {
                    OutlineMaskPreference::NONE
                },
                picking_layer_id: PickingLayerId {
                    object: PickingLayerObjectId(u64::from(body)),
                    instance: PickingLayerInstanceId(u64::from(number)),
                },
                cull_mode: None,
            });
        }
        Ok(())
    }
}

/// Each edge of body `body`'s `mesh` of two points or more as a line strip, placed in the
/// view in f64 ([`View::place`]: each point's offset from the camera's point and its
/// rotation into the view, so precise across the view at every distance and angle), built
/// for each view: its picking ID the body's number and the edge's with [`EDGE_BIT`]; a
/// highlighted one (in `lit`) wider, in the highlight's blue, in [`Highlight::Tint`], or
/// outlined, as it is otherwise, in [`Highlight::Outline`].
fn edges(lines: &mut LineDrawableBuilder<'_>, body: u32, mesh: &Mesh, lit: &Lit, view: &View) {
    let mut batch = lines
        .batch(format!("Body{body}"))
        .world_from_obj(glam::Affine3A::IDENTITY)
        .picking_object_id(PickingLayerObjectId(u64::from(body)))
        .depth_offset(EDGE_DEPTH_OFFSET);
    let mut first = 0usize;
    for (edge, &count) in mesh.edge_point_counts.iter().enumerate() {
        // The check makes the counts add up, each a u32: as a usize, at most the points'.
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        let points = mesh
            .edge_points
            .get(first..first.saturating_add(count))
            .unwrap_or(&[]);
        first = first.saturating_add(count);
        let Ok(number) = u32::try_from(edge + 1) else {
            break;
        };
        if points.len() < 2 {
            continue;
        }
        let highlighted = lit.edge(number);
        let (width, colour, outline) = match (highlighted, lit.style) {
            (true, Highlight::Tint) => (
                HIGHLIGHTED_EDGE_WIDTH,
                HIGHLIGHT,
                OutlineMaskPreference::NONE,
            ),
            (true, Highlight::Outline) => (EDGE_WIDTH, EDGE, outlined()),
            (false, _) => (EDGE_WIDTH, EDGE, OutlineMaskPreference::NONE),
        };
        batch
            .add_strip(points.iter().map(|p| view.place(*p)))
            .radius(Size::new_ui_points(width / 2.0))
            .color(Color32::from_rgb(colour[0], colour[1], colour[2]))
            .picking_instance_id(PickingLayerInstanceId(u64::from(EDGE_BIT | number)))
            .outline_mask_ids(outline);
    }
}

/// A box: its lowest and highest corners.
type Bounds = ([f64; 3], [f64; 3]);

/// The box of the positions of the face `span`'s triangles; none for a face of none.
fn face_bounds(mesh: &Mesh, span: FaceSpan) -> Option<Bounds> {
    let triangles = mesh.triangles.get(span.first..span.first + span.count)?;
    bounds_of(
        triangles
            .iter()
            .flatten()
            .filter_map(|&v| mesh.positions.get(usize::try_from(v).ok()?)),
    )
}

/// The box of `points`; none for none.
fn bounds_of<'a>(mut points: impl Iterator<Item = &'a [f64; 3]>) -> Option<Bounds> {
    let &start = points.next()?;
    Some(points.fold((start, start), |(mut low, mut high), p| {
        for k in 0..3 {
            low[k] = low[k].min(p[k]);
            high[k] = high[k].max(p[k]);
        }
        (low, high)
    }))
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

/// The face `span` of body `body`'s `mesh` as a mesh of its own about `anchor`
/// ([`face_vertices`]), coloured plain or tinted.
fn face_cpu_mesh(
    ctx: &RenderContext,
    body: u32,
    mesh: &Mesh,
    anchor: [f64; 3],
    span: FaceSpan,
    tinted: bool,
) -> Result<CpuMesh, String> {
    let FaceVertices {
        positions,
        normals,
        triangles,
    } = face_vertices(mesh, span, anchor)?;
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
        vertex_colors: vec![vertex_colour(tinted); vertices],
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

#[cfg(test)]
mod frame_tests;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use silico_protocol::{
        Camera, Frame, Mesh, Projection, RenderAdapter, RenderBody, RenderScene,
    };

    use super::{
        Bounds, OUTPUT_FORMAT, Renderer, Uploaded, View, centre_of, device, face_vertices, unit,
        vec3, world_from_mesh,
    };
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
        // while h 2^-22 × sec² ≤ an eighth of a pixel there.
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
            let view = View::of(&scene.camera, &scene).unwrap();
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
            let view = View::of(&scene.camera, &scene).unwrap();
            let want = if about_eye { [0.0, -d, 0.0] } else { [0.0; 3] };
            assert_eq!(view.anchor(segment), want, "{d}");
        }
        // An eye in the face's box is about the eye, at any size; a 2 µm face a millimetre
        // beside it is about its centre.
        let scene = looking_along_y([0.0, 0.0, 0.0], 90.0, 64);
        let view = View::of(&scene.camera, &scene).unwrap();
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
                // A highlight uploads the face's tinted mesh and drops its plain one; no view
                // after it uploads; its end uploads the plain one again.
                let mut lit = front.clone();
                lit.bodies[0].highlight_faces = vec![2];
                assert_eq!(uploads_and_centre(r, &lit), (1, [1, 3]));
                lit.camera = top.camera.clone();
                assert_eq!(uploads_and_centre(r, &lit), (0, [1, 6]));
                let face = &r.bodies[&1].faces[2];
                assert!(face.tinted.is_some() && face.plain.is_none());
                assert_eq!(uploads_and_centre(r, &top), (1, [1, 6]));
                // A body whose mesh changed is uploaded again; one beside it is new.
                let mut moved = front.clone();
                moved.bodies[0].mesh = box_mesh([1.0, 0.0, 0.0], [40.0, 20.0, 10.0]);
                moved
                    .bodies
                    .push(body(2, box_mesh([100.0, 0.0, 0.0], [1.0; 3])));
                assert_eq!(uploads_and_centre(r, &moved), (12, [1, 3]));
                // Only the last scene's bodies are kept.
                assert_eq!(r.bodies.keys().copied().collect::<Vec<_>>(), [1, 2]);
                assert_eq!(uploads_and_centre(r, &front), (6, [1, 3]));
                assert_eq!(r.bodies.keys().copied().collect::<Vec<_>>(), [1]);
                // A face highlighted the first time it's drawn is uploaded tinted alone.
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
                assert_eq!(uploads_and_centre(r, &view(0.5)), (6, [1, 3]));
                assert_eq!(uploads_and_centre(r, &view(0.51)), (0, [1, 3]));
                assert_eq!(uploads_and_centre(r, &view(0.49)), (0, [1, 3]));
            },
        );
    }

    #[test]
    fn a_kilometre_rod_seen_at_its_end_moves_its_four_long_faces_alone_and_draws_its_end() {
        // (b) A rod 1e6 × 20 × 10 from the origin, the eye 5 in front of its end and x mm
        // right of it, 0.5° across, 64 square. Its long faces (h ≈ 5e5, rounding 0.12 mm)
        // fail at the 8.5e-5 mm an eighth of a pixel is there (5 × 2 tan 0.25° / 64 / 8):
        // about the eye, uploaded again each view the eye moves. Its end caps (h = √125,
        // rounding 2.7e-6 mm) stay about their centres. By hand the end, 0.01 mm left of
        // the centre, is 0.01 / 6.817e-4 = 14.7 pixels left of it, at column 17.3: column
        // 14 is the background and columns 21 on are the front face (Face3).
        on_lavapipe(
            "a_kilometre_rod_seen_at_its_end_moves_its_four_long_faces_alone_and_draws_its_end",
            |r| {
                let rod = box_mesh([0.0; 3], [1e6, 20.0, 10.0]);
                let view = |x: f64| RenderScene {
                    bodies: vec![body(1, rod.clone())],
                    ..looking_along_y([x, -5.0, 5.0], 0.5, 64)
                };
                assert_eq!(uploads_and_centre(r, &view(0.01)).0, 6);
                let row = |ids: &[[u32; 2]]| ids[32 * 64..33 * 64].to_vec();
                let drawn = row(&ids(r, &view(0.01)));
                assert_eq!(drawn[14], [0, 0], "{drawn:?}");
                assert!(drawn[21..].iter().all(|id| *id == [1, 3]), "{drawn:?}");
                for x in [0.011, 0.012] {
                    assert_eq!(uploads_and_centre(r, &view(x)).0, 4, "{x}");
                }
                let moved: Vec<u32> = r.bodies[&1]
                    .faces
                    .iter()
                    .filter(|f| f.anchor == [0.012, -5.0, 5.0])
                    .map(|f| f.span.number)
                    .collect();
                assert_eq!(moved, [3, 4, 5, 6]);
            },
        );
    }

    /// A plate: Face1 a kilometre long (x 0 to 1000, z 0 to 1, at y = 0), and Face2 a
    /// millimetre square in front of it (x and z 0 to 1, at y = −0.5).
    fn plate() -> Mesh {
        let quad = |corners: [[f64; 3]; 4]| corners;
        let faces = [
            quad([
                [0.0, 0.0, 0.0],
                [1000.0, 0.0, 0.0],
                [1000.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
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
        // half diagonal is √(500² + 0.5²), so it stays about its centre from the view height
        // h × 64 / 2^19 = 0.061035 mm up. A pan at 1.001 times that uploads nothing; at
        // 0.999 times it, Face1 alone is uploaded again about the new target. Face2 is at the
        // centre throughout.
        on_lavapipe(
            "an_orthographic_long_face_moves_with_the_target_only_past_an_eighth_of_a_pixel",
            |r| {
                let least = (500f64 * 500.0 + 0.25).sqrt() * 64.0 / 524_288.0;
                assert!((least - 0.061_035).abs() < 1e-6, "{least}");
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
    fn an_eye_inside_a_tube_moves_its_two_walls_alone_and_never_its_ends() {
        // (d) A tube of radii 5 and 10, 100 long, the eye in its bore at z = 50 looking along
        // +z, 60°, 64 square. Both walls' boxes hold the eye: about the eye, uploaded again
        // each view the eye moves. The ends (h = 10√2, rounding 3.4e-6 mm, sec² 1 + tan² 30°
        // × 2 = 5/3) are 50 away, where an eighth of a pixel is 50 × 2 tan 30° / 64 / 8 =
        // 0.113 mm: about their centres, never uploaded again. The centre looks out through
        // the far end; the inside wall is seen, the outside never.
        on_lavapipe(
            "an_eye_inside_a_tube_moves_its_two_walls_alone_and_never_its_ends",
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
}
