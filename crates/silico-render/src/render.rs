//! A [`RenderScene`] drawn headless (docs/design/m4.6-render.md, as docs/design/m5.0-app.md
//! section 10 amends it): its bodies' faces and edges, its highlights in Fusion's selection
//! blue, read back as an 8-bit sRGB PNG and, when asked, the ID buffer.
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
//! [`Renderer::new`] draws on any (the app's, later, eframe's).

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
    Camera, EDGE_BIT, Frame, Mesh, PixelId, Projection, RenderBody, RenderScene,
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
/// Edges' widths in pixels: every edge, dark and solid, and a highlighted one.
pub const EDGE_WIDTH: f32 = 2.0;
pub const HIGHLIGHTED_EDGE_WIDTH: f32 = 4.0;
/// A highlighted face's outline, in pixels.
pub const OUTLINE_WIDTH: f32 = 1.5;
/// The format of the target a view is composited into: the worker reads its image from the
/// view's own target, so this is only the render context's.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
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

/// Why a render failed.
#[derive(Debug)]
pub enum RenderFailure {
    /// The GPU (lavapipe's, here: the machine's memory) ran out of memory.
    OutOfMemory(String),
    /// Anything else: the renderer's own fault.
    Failed(String),
}

impl std::fmt::Display for RenderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderFailure::OutOfMemory(what) => write!(f, "out of GPU memory: {what}"),
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

/// A render context on one device, kept between renders, with the bodies it last drew
/// uploaded.
pub struct Renderer {
    ctx: RenderContext,
    /// The next render's readbacks' identifiers (each render takes two), so a render that
    /// failed never leaves a readback a later one could take as its own, for a caller that
    /// keeps the renderer after a failure (the worker doesn't; the bench could).
    readback: u64,
    /// The last render's bodies, by number, as uploaded: a body the next render has again
    /// with the same mesh is drawn from them; the others are dropped before it uploads
    /// anything, and all of them after a render that failed.
    bodies: BTreeMap<u32, Uploaded>,
    /// How many face meshes it has uploaded.
    uploads: u64,
    /// How long the last render took to place its faces and edges for its view, on the CPU.
    placed: Duration,
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
        })
    }

    /// How many face meshes it has uploaded since it was made: a face's, plain or in the
    /// highlight's tint, the first time it's drawn so, and again after its body's mesh, the
    /// point it's drawn about ([`View::anchor`]), or its highlight changed.
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    /// How long the last render took to place its faces and edges for its view on the CPU
    /// (each face's anchor and transform, its instance, and the edges' strips), uploads
    /// included: for the bench, never a result.
    pub fn placed(&self) -> Duration {
        self.placed
    }

    /// `scene` drawn. The scene must have passed [`RenderScene::check`]. Errors the GPU
    /// reports while it draws (out of memory, or anything wgpu finds invalid) fail the
    /// render rather than leave a blank image.
    pub fn render(&mut self, scene: &RenderScene) -> Result<Rendered, RenderFailure> {
        let _span = tracing::debug_span!(
            "render",
            bodies = scene.bodies.len(),
            width = scene.width,
            height = scene.height
        )
        .entered();
        let device = self.ctx.device.clone();
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validity = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let ids = (self.readback, self.readback + 1);
        self.readback += 2;
        let drawn = self.draw(scene, ids);
        let invalid = pollster::block_on(validity.pop());
        let short = pollster::block_on(memory.pop());
        let result = if let Some(e) = short {
            Err(RenderFailure::OutOfMemory(e.to_string()))
        } else if let Some(e) = invalid {
            Err(RenderFailure::Failed(format!(
                "the GPU found the render invalid: {e}"
            )))
        } else {
            drawn.map_err(RenderFailure::Failed)
        };
        if result.is_err() {
            // Nothing vouches for buffers a failed render made: none is drawn again.
            self.bodies.clear();
        }
        result
    }

    /// `scene` drawn, its readbacks identified by `(picking, screenshot)`.
    fn draw(
        &mut self,
        scene: &RenderScene,
        (picking, screenshot): (u64, u64),
    ) -> Result<Rendered, String> {
        let Renderer {
            ctx,
            bodies: held,
            uploads,
            placed,
            ..
        } = self;
        ctx.begin_frame();
        let view = View::of(&scene.camera, scene)?;
        let placing = std::time::Instant::now();
        // What this scene doesn't draw again is dropped before anything is uploaded: the
        // bodies it hasn't with the same mesh, and of those it has, each face's mesh about
        // another point or in the other colour. So the GPU holds about one scene's meshes.
        let mut kept = std::mem::take(held);
        kept.retain(|number, uploaded| {
            scene
                .bodies
                .binary_search_by_key(number, |b| b.body)
                .is_ok_and(|i| scene.bodies[i].mesh == uploaded.mesh)
        });
        let mut drawn = Vec::with_capacity(scene.bodies.len());
        for body in &scene.bodies {
            let mut uploaded = match kept.remove(&body.body) {
                Some(uploaded) => uploaded,
                None => Uploaded::new(body)?,
            };
            uploaded.settle(&view, body);
            drawn.push(uploaded);
        }
        let mut instances = Vec::new();
        let mut lines = LineDrawableBuilder::new(ctx);
        lines.enable_alpha_blending();
        for (body, uploaded) in scene.bodies.iter().zip(&mut drawn) {
            uploaded.faces(ctx, body, &view, &mut instances, uploads)?;
            edges(&mut lines, body, &view);
        }
        *held = scene.bodies.iter().map(|b| b.body).zip(drawn).collect();
        *placed = placing.elapsed();
        let highlighted = scene
            .bodies
            .iter()
            .any(|b| !b.highlight_faces.is_empty() || !b.highlight_edges.is_empty());
        let config = TargetConfiguration {
            name: "render".into(),
            resolution_in_pixel: [scene.width, scene.height],
            // The camera at the world's origin, looking along −z with +y up: the bodies are
            // moved into its view.
            view_from_world: macaw::IsoTransform::IDENTITY,
            projection_from_view: view.projection,
            outline_config: highlighted.then(|| OutlineConfig {
                outline_radius_pixel: OUTLINE_WIDTH,
                color_layer_a: srgb(HIGHLIGHT),
                color_layer_b: Rgba::TRANSPARENT,
            }),
            picking_config: scene.ids.then(|| ViewPickingConfiguration {
                picking_rect: RectInt {
                    min: glam::IVec2::ZERO,
                    extent: glam::UVec2::new(scene.width, scene.height),
                },
                readback_identifier: picking,
                show_debug_view: false,
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
            // Its borrow of the context ends here, before the frame is submitted.
            drop(lines);
        }
        builder
            .schedule_screenshot(ctx, screenshot, ())
            .map_err(|e| format!("couldn't schedule the image's readback: {e}"))?;
        let commands = builder
            .draw(ctx, srgb(BACKGROUND))
            .map_err(|e| format!("couldn't draw: {e}"))?;
        ctx.before_submit();
        ctx.queue.submit([commands]);
        read_back(ctx, scene, picking, screenshot)
    }
}

/// The view as the fixed camera draws it: the camera's frame in f64, the point the bodies
/// are moved about, and the projection.
struct View {
    frame: Frame,
    /// The point the camera is at, in the document: the eye for a perspective view; for an
    /// orthographic one, the target, the camera then `depth` behind it.
    origin: [f64; 3],
    depth: f64,
    projection: ViewProjection,
    /// How tall the image is, in mm: across the whole view (orthographic), or for each mm
    /// from the eye (perspective, `2 tan(fov / 2)`).
    span: f64,
    perspective: bool,
    /// The image's height in pixels.
    height: u32,
    /// How much more a position's error moves the image at its corner than at its centre, in
    /// perspective: the corner ray's sec², 1 + tan²(fov/2)(1 + aspect²); 1 orthographic.
    corner: f64,
}

impl View {
    fn of(camera: &Camera, scene: &RenderScene) -> Result<View, String> {
        let frame = camera.frame().map_err(|e| e.message)?;
        let view = match camera.projection {
            Projection::Orthographic { view_height } => {
                // Only the eye's direction counts: the camera is set past every point of the
                // scene, so nothing is cut off behind the eye (FreeCAD fits its clipping
                // planes to the scene, too), and the far plane past the scene's other side.
                let reach = scene.reach(camera.target);
                let margin = (reach * CLEARANCE).max(1.0);
                View {
                    frame,
                    origin: camera.target,
                    depth: reach + margin,
                    span: view_height,
                    perspective: false,
                    height: scene.height,
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
                let reach = scene.reach(camera.eye).max(frame.distance);
                let tan = libm::tan(field_of_view.to_radians() / 2.0);
                let aspect = f64::from(scene.width) / f64::from(scene.height);
                View {
                    frame,
                    origin: camera.eye,
                    depth: 0.0,
                    span: 2.0 * tan,
                    perspective: true,
                    height: scene.height,
                    corner: 1.0 + tan * tan * (1.0 + aspect * aspect),
                    projection: ViewProjection::Perspective {
                        vertical_fov: field_of_view.to_radians() as f32,
                        near_plane_distance: (reach * NEAR_PLANE_OF_REACH) as f32,
                        aspect_ratio: scene.width as f32 / scene.height as f32,
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
    frame: &Frame,
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

/// A normal of unit length, scaled by its largest part first so a huge or tiny one neither
/// overflows nor underflows (re_renderer normalizes in f32); none for zero, which
/// re_renderer draws unlit.
fn unit(n: [f32; 3]) -> glam::Vec3 {
    let n = n.map(f64::from);
    let largest = n.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    if largest == 0.0 {
        return glam::Vec3::ZERO;
    }
    let scaled = n.map(|c| c / largest);
    let length = dot(scaled, scaled).sqrt();
    vec3(scaled.map(|c| c / length))
}

/// A body as uploaded: what it was made from, to know it again, and its faces.
struct Uploaded {
    mesh: Mesh,
    faces: Vec<UploadedFace>,
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
    /// `body`'s faces with triangles, each about its box's centre, none uploaded yet.
    fn new(body: &RenderBody) -> Result<Uploaded, String> {
        let mesh = &body.mesh;
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
            let Some(bounds) = face_bounds(mesh, span) else {
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
            mesh: mesh.clone(),
            faces,
        })
    }

    /// Each face's point for `view`, and what it won't draw dropped: its meshes about
    /// another point, and the colour `body` doesn't draw it in.
    fn settle(&mut self, view: &View, body: &RenderBody) {
        for face in &mut self.faces {
            let anchor = view.anchor(face.bounds);
            if anchor != face.anchor {
                face.anchor = anchor;
                face.plain = None;
                face.tinted = None;
            }
            if highlights(body, face.span.number) {
                face.plain = None;
            } else {
                face.tinted = None;
            }
        }
    }

    /// Each face as a mesh instance of its own, moved into `view` by its own transform, its
    /// picking ID the body's number and the face's; a highlighted one tinted and outlined.
    /// A face's mesh is uploaded where it has none (counted in `uploads`).
    fn faces(
        &mut self,
        ctx: &RenderContext,
        body: &RenderBody,
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
        let _span = (new > 0)
            .then(|| tracing::debug_span!("upload", body = body.body, faces = new).entered());
        for face in &mut self.faces {
            let number = face.span.number;
            let highlighted = highlights(body, number);
            let held = if highlighted {
                &mut face.tinted
            } else {
                &mut face.plain
            };
            let gpu_mesh = match held {
                Some(mesh) => Arc::clone(mesh),
                None => {
                    let cpu = face_cpu_mesh(ctx, body, face.anchor, face.span, highlighted)?;
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
                    OutlineMaskPreference::some(1, 0)
                } else {
                    OutlineMaskPreference::NONE
                },
                picking_layer_id: PickingLayerId {
                    object: PickingLayerObjectId(u64::from(body.body)),
                    instance: PickingLayerInstanceId(u64::from(number)),
                },
                cull_mode: None,
            });
        }
        Ok(())
    }
}

/// Whether `body` highlights its face numbered `number` from 1.
fn highlights(body: &RenderBody, number: u32) -> bool {
    body.highlight_faces.binary_search(&(number - 1)).is_ok()
}

/// Each edge of `body` of two points or more as a line strip, placed in the view in f64
/// ([`View::place`]: each point's offset from the camera's point and its rotation into the
/// view, so precise across the view at every distance and angle), built for each view:
/// its picking ID the body's number and the edge's with [`EDGE_BIT`]; a highlighted one
/// wider, in the highlight's blue.
fn edges(lines: &mut LineDrawableBuilder<'_>, body: &RenderBody, view: &View) {
    let mesh = &body.mesh;
    let mut batch = lines
        .batch(format!("Body{}", body.body))
        .world_from_obj(glam::Affine3A::IDENTITY)
        .picking_object_id(PickingLayerObjectId(u64::from(body.body)))
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
        let highlighted = body.highlight_edges.binary_search(&(number - 1)).is_ok();
        let (width, colour) = if highlighted {
            (HIGHLIGHTED_EDGE_WIDTH, HIGHLIGHT)
        } else {
            (EDGE_WIDTH, EDGE)
        };
        batch
            .add_strip(points.iter().map(|p| view.place(*p)))
            .radius(Size::new_ui_points(width / 2.0))
            .color(Color32::from_rgb(colour[0], colour[1], colour[2]))
            .picking_instance_id(PickingLayerInstanceId(u64::from(EDGE_BIT | number)));
    }
}

/// A box: its lowest and highest corners.
type Bounds = ([f64; 3], [f64; 3]);

/// The box of the positions of the face `span`'s triangles; none for a face of none.
fn face_bounds(mesh: &Mesh, span: FaceSpan) -> Option<Bounds> {
    let triangles = mesh.triangles.get(span.first..span.first + span.count)?;
    let mut points = triangles
        .iter()
        .flatten()
        .filter_map(|&v| mesh.positions.get(usize::try_from(v).ok()?));
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

/// The face `span` of `body` as a mesh of its own about `anchor` ([`face_vertices`]),
/// coloured plain or tinted.
fn face_cpu_mesh(
    ctx: &RenderContext,
    body: &RenderBody,
    anchor: [f64; 3],
    span: FaceSpan,
    tinted: bool,
) -> Result<CpuMesh, String> {
    let FaceVertices {
        positions,
        normals,
        triangles,
    } = face_vertices(&body.mesh, span, anchor)?;
    let vertices = positions.len();
    Ok(CpuMesh {
        label: format!("Body{}Face{}", body.body, span.number).into(),
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
mod tests {
    use silico_protocol::{
        Camera, Frame, Mesh, Projection, RenderAdapter, RenderBody, RenderScene,
    };

    use super::{
        Bounds, OUTPUT_FORMAT, Renderer, Uploaded, View, centre_of, device, face_vertices, vec3,
        world_from_mesh,
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
        let uploaded = Uploaded::new(&body(1, mesh.clone())).unwrap();
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
        let uploaded = Uploaded::new(&body(1, mesh)).unwrap();
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
        let uploaded = Uploaded::new(&body(1, point)).unwrap();
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
    /// the process runs).
    fn on_lavapipe(name: &str, test: impl FnOnce(&mut Renderer)) {
        if let Some(vars) = crate::lavapipe::environment()
            && vars
                .iter()
                .any(|(k, v)| std::env::var(k).map_or(true, |have| have != *v))
        {
            let name = format!("render::tests::{name}");
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
