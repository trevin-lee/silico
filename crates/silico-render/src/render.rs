//! A [`RenderScene`] drawn headless (docs/design/m4.6-render.md, as docs/design/m5.0-app.md
//! section 10 amends it): its bodies' faces and edges, its highlights in Fusion's selection
//! blue, read back as an 8-bit sRGB PNG and, when asked, the ID buffer.
//!
//! The camera stays fixed in the world and the bodies move. Each body's mesh is uploaded
//! once, its positions about its own box's centre (subtracted in f64 before they're f32) and
//! its normals its own, and each view moves it into place by its instance's transform: the
//! view's rotation and the centre's offset from the eye, computed in f64 and then f32
//! ([`world_from_mesh`]). So a new view of the same bodies uploads nothing: it builds only
//! the instances and the edges' line strips, whose transform is batch data. The light is
//! `re_renderer`'s own (an ambient and two lights fixed in the world), per pixel, which the
//! fixed camera makes the camera's.
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
    /// with the same mesh is drawn from them; the others are dropped.
    bodies: BTreeMap<u32, Uploaded>,
    /// How many face meshes it has uploaded.
    uploads: u64,
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
        })
    }

    /// How many face meshes it has uploaded since it was made: a face's, and a highlighted
    /// face's in its tint, once each until its body's mesh changes.
    pub fn uploads(&self) -> u64 {
        self.uploads
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
        if let Some(e) = short {
            return Err(RenderFailure::OutOfMemory(e.to_string()));
        }
        if let Some(e) = invalid {
            return Err(RenderFailure::Failed(format!(
                "the GPU found the render invalid: {e}"
            )));
        }
        drawn.map_err(RenderFailure::Failed)
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
            ..
        } = self;
        ctx.begin_frame();
        let view = View::of(&scene.camera, scene)?;
        let mut kept = BTreeMap::new();
        let mut instances = Vec::new();
        let mut lines = LineDrawableBuilder::new(ctx);
        lines.enable_alpha_blending();
        for body in &scene.bodies {
            let mut uploaded = match held.remove(&body.body) {
                Some(uploaded) if uploaded.mesh == body.mesh => uploaded,
                _ => Uploaded::new(body)?,
            };
            let world_from_mesh = view.world_from_mesh(uploaded.centre);
            uploaded.faces(ctx, body, world_from_mesh, &mut instances, uploads)?;
            uploaded.edges(&mut lines, body, world_from_mesh);
            kept.insert(body.body, uploaded);
        }
        // Only this scene's bodies are kept: what the next view of it needs, no more.
        *held = kept;
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
                View {
                    frame,
                    origin: camera.eye,
                    depth: 0.0,
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

    fn world_from_mesh(&self, centre: [f64; 3]) -> glam::Affine3A {
        world_from_mesh(&self.frame, self.origin, self.depth, centre)
    }
}

/// Where a mesh stored about `centre` is drawn, for the camera at `origin` (then `depth`
/// back along the view) with `frame`: the view's rotation (its right, up, and back as the
/// world's x, y, and z, where the fixed camera looks along −z with +y up), and the centre
/// about the camera in the camera's frame, `R (centre − origin) − (0, 0, depth)`, in f64
/// and then f32. A body far from the origin keeps its precision: the offset is taken in f64
/// before anything is f32, and what's f32 is the size of the view, not of the document.
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

/// A body as uploaded: what it was made from, to know it again, the centre its positions
/// are about, its faces' meshes, and its edges' strips.
struct Uploaded {
    mesh: Mesh,
    centre: [f64; 3],
    faces: Vec<UploadedFace>,
    /// Each edge of two points or more: its number from 1 and its points about the centre.
    strips: Vec<(u32, Vec<glam::Vec3>)>,
}

/// A face with triangles: which it is, and its mesh, plain and tinted, each uploaded the
/// first time a render draws the face so.
struct UploadedFace {
    span: FaceSpan,
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
    /// `body`'s faces, to be uploaded as each is drawn, and its edges' strips, about the
    /// centre of the box of its positions and edge points (the origin if it has none).
    fn new(body: &RenderBody) -> Result<Uploaded, String> {
        let mesh = &body.mesh;
        let centre = centre(mesh);
        let mut faces = Vec::new();
        let mut first = 0usize;
        for (face, &count) in mesh.face_triangles.iter().enumerate() {
            let count = usize::try_from(count).map_err(|e| e.to_string())?;
            let at = first;
            first += count;
            if count == 0 {
                continue;
            }
            let span = FaceSpan {
                number: u32::try_from(face + 1).map_err(|e| e.to_string())?,
                first: at,
                count,
            };
            faces.push(UploadedFace {
                span,
                plain: None,
                tinted: None,
            });
        }
        let mut strips = Vec::new();
        let mut first = 0usize;
        for (edge, &count) in mesh.edge_point_counts.iter().enumerate() {
            let count = usize::try_from(count).map_err(|e| e.to_string())?;
            let points = &mesh.edge_points[first..first + count];
            first += count;
            if points.len() < 2 {
                continue;
            }
            let number = u32::try_from(edge + 1).map_err(|e| e.to_string())?;
            strips.push((
                number,
                points.iter().map(|p| vec3(sub(*p, centre))).collect(),
            ));
        }
        Ok(Uploaded {
            mesh: mesh.clone(),
            centre,
            faces,
            strips,
        })
    }

    /// Each face as a mesh instance of its own, moved by `world_from_mesh`, its picking ID
    /// the body's number and the face's; a highlighted one tinted and outlined. A face's
    /// mesh, plain or tinted, is uploaded the first time it's drawn so (counted in
    /// `uploads`).
    fn faces(
        &mut self,
        ctx: &RenderContext,
        body: &RenderBody,
        world_from_mesh: glam::Affine3A,
        instances: &mut Vec<GpuMeshInstance>,
        uploads: &mut u64,
    ) -> Result<(), String> {
        let centre = self.centre;
        let new = self.faces.iter().filter(|f| f.plain.is_none()).count();
        let _span = (new > 0)
            .then(|| tracing::debug_span!("upload", body = body.body, faces = new).entered());
        for face in &mut self.faces {
            let number = face.span.number;
            let highlighted = body.highlight_faces.binary_search(&(number - 1)).is_ok();
            let held = if highlighted {
                &mut face.tinted
            } else {
                &mut face.plain
            };
            let gpu_mesh = match held {
                Some(mesh) => Arc::clone(mesh),
                None => {
                    let mesh = face_mesh(ctx, body, centre, face.span, highlighted)?;
                    *uploads += 1;
                    Arc::clone(held.insert(mesh))
                }
            };
            instances.push(GpuMeshInstance {
                gpu_mesh,
                world_from_mesh,
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

    /// Each edge as a line strip, its batch moved by `world_from_mesh`, its picking ID the
    /// body's number and the edge's with [`EDGE_BIT`]; a highlighted one wider, in the
    /// highlight's blue.
    fn edges(
        &self,
        lines: &mut LineDrawableBuilder<'_>,
        body: &RenderBody,
        world_from_mesh: glam::Affine3A,
    ) {
        let mut batch = lines
            .batch(format!("Body{}", body.body))
            .world_from_obj(world_from_mesh)
            .picking_object_id(PickingLayerObjectId(u64::from(body.body)))
            .depth_offset(EDGE_DEPTH_OFFSET);
        for (number, points) in &self.strips {
            let highlighted = body.highlight_edges.binary_search(&(number - 1)).is_ok();
            let (width, colour) = if highlighted {
                (HIGHLIGHTED_EDGE_WIDTH, HIGHLIGHT)
            } else {
                (EDGE_WIDTH, EDGE)
            };
            batch
                .add_strip(points.iter().copied())
                .radius(Size::new_ui_points(width / 2.0))
                .color(Color32::from_rgb(colour[0], colour[1], colour[2]))
                .picking_instance_id(PickingLayerInstanceId(u64::from(EDGE_BIT | number)));
        }
    }
}

/// The centre of the box of `mesh`'s positions and edge points, in f64; the origin for a mesh
/// of neither.
fn centre(mesh: &Mesh) -> [f64; 3] {
    let mut points = mesh.positions.iter().chain(&mesh.edge_points);
    let Some(&start) = points.next() else {
        return [0.0; 3];
    };
    let (low, high) = points.fold((start, start), |(mut low, mut high), p| {
        for k in 0..3 {
            low[k] = low[k].min(p[k]);
            high[k] = high[k].max(p[k]);
        }
        (low, high)
    });
    // Halved first, so the sum can't overflow.
    std::array::from_fn(|k| low[k] / 2.0 + high[k] / 2.0)
}

/// The face `span` of `body` uploaded as a mesh of its own about `centre`: its own vertices
/// renumbered from 0, their normals of unit length, coloured plain or tinted.
fn face_mesh(
    ctx: &RenderContext,
    body: &RenderBody,
    centre: [f64; 3],
    span: FaceSpan,
    tinted: bool,
) -> Result<Arc<GpuMesh>, String> {
    let FaceSpan {
        number,
        first,
        count,
    } = span;
    let mesh = &body.mesh;
    let triangles = &mesh.triangles[first..first + count];
    let mut index = BTreeMap::new();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut renumbered = Vec::with_capacity(triangles.len());
    for t in triangles {
        let mut out = [0u32; 3];
        for (k, &v) in t.iter().enumerate() {
            let next = u32::try_from(index.len()).map_err(|e| e.to_string())?;
            let i = usize::try_from(v).map_err(|e| e.to_string())?;
            out[k] = *index.entry(v).or_insert_with(|| {
                positions.push(vec3(sub(mesh.positions[i], centre)));
                normals.push(unit(mesh.normals[i]));
                next
            });
        }
        renumbered.push(glam::UVec3::from(out));
    }
    let vertices = positions.len();
    let cpu = CpuMesh {
        label: format!("Body{}Face{number}", body.body).into(),
        triangle_indices: renumbered,
        vertex_colors: vec![vertex_colour(tinted); vertices],
        vertex_normals: normals,
        vertex_texcoords: vec![glam::Vec2::ZERO; vertices],
        bbox: macaw::BoundingBox::from_points(positions.iter().copied()),
        vertex_positions: positions,
        materials: smallvec::smallvec![Material {
            label: "body".into(),
            index_range: re_renderer::Span {
                start: 0,
                len: u32::try_from(triangles.len() * 3).map_err(|e| e.to_string())?,
            },
            albedo: ctx.texture_manager_2d.white_texture_unorm_handle().clone(),
            albedo_factor: Rgba::WHITE,
        }],
    };
    let gpu = GpuMesh::new(ctx, &cpu).map_err(|e| format!("Face{number}: {e}"))?;
    Ok(Arc::new(gpu))
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
    use silico_protocol::{Camera, Frame, Projection, RenderAdapter, RenderScene};

    use super::{OUTPUT_FORMAT, Renderer, View, centre, device, sub, vec3, world_from_mesh};
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

    #[test]
    fn a_bodys_transform_is_the_views_rotation_and_its_centre_about_the_eye() {
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
        assert_near(
            p,
            [10.0 / r2, 4.0 / r6, 2.0 / r3 + 1.0],
            1e-6,
            "a point along back",
        );
        // An orthographic view's camera sits `depth` back from its target: 30 farther along
        // −z.
        let m = world_from_mesh(&oblique(), [10.0, 0.0, 20.0], 30.0, [15.0, 5.0, 22.0]);
        assert_near(
            glam::Vec3::from(m.translation),
            [10.0 / r2, 4.0 / r6, 2.0 / r3 - 30.0],
            1e-5,
            "orthographic",
        );
    }

    #[test]
    fn a_body_far_from_the_origin_keeps_its_offset_from_the_eye_and_its_mesh_its_shape() {
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
        assert_eq!(
            vec3(eye).x,
            vec3(centre_at).x,
            "f32 keeps the offset after all"
        );
        // The mesh about its own centre: a box 0.5 wide there, its corners ±0.25 about it in
        // f32 exactly, where in f32 about the origin they'd be one number.
        let mut mesh = box_mesh([1e9, 2e9 - 0.25, -1e9 + 0.25], [0.5, 0.5, 0.5]);
        mesh.edge_points.clear();
        mesh.edge_point_counts.clear();
        assert_eq!(centre(&mesh), centre_at);
        assert_eq!(
            vec3(sub(mesh.positions[0], centre_at)),
            glam::Vec3::new(-0.25, -0.25, -0.25)
        );
        assert_eq!(vec3(mesh.positions[0]), vec3([1e9 + 0.5, 2e9, -1e9]));
    }

    #[test]
    fn a_mesh_of_no_points_is_about_the_origin_and_one_point_about_itself() {
        let mut mesh = box_mesh([0.0; 3], [1.0; 3]);
        mesh.positions.clear();
        mesh.edge_points.clear();
        assert_eq!(centre(&mesh), [0.0; 3]);
        mesh.edge_points = vec![[1e12, -1e12, 3.0]];
        assert_eq!(centre(&mesh), [1e12, -1e12, 3.0]);
        // The box's corners at the largest coordinates: halved before they're summed.
        mesh.edge_points.clear();
        mesh.positions = vec![[1e12, 1e12, 1e12], [-1e12, 1e12, -1e12]];
        assert_eq!(centre(&mesh), [0.0, 1e12, 0.0]);
        mesh.positions = vec![[f64::MAX, f64::MAX, 1.0], [f64::MAX, -f64::MAX, 1.0]];
        assert_eq!(centre(&mesh), [f64::MAX, 0.0, 1.0]);
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
        let perspective = RenderScene {
            camera: Camera {
                projection: Projection::Perspective {
                    field_of_view: 30.0,
                },
                ..scene.camera
            },
            ..scene
        };
        let view = View::of(&perspective.camera, &perspective).unwrap();
        assert_eq!((view.origin, view.depth), (perspective.camera.eye, 0.0));
    }

    /// `test` run on lavapipe in this process: in a run of this test alone with lavapipe's
    /// variables, if this process hasn't them (a library's loader path can't be set once
    /// the process runs).
    fn on_lavapipe(name: &str, test: impl FnOnce(&wgpu::Adapter)) {
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
        test(&adapters[0]);
    }

    /// The ID at the image's centre.
    fn centre_id(renderer: &mut Renderer, scene: &RenderScene) -> [u32; 2] {
        let ids = renderer.render(scene).unwrap().ids.unwrap();
        ids[usize::try_from(scene.height / 2 * scene.width + scene.width / 2).unwrap()]
    }

    #[test]
    fn a_new_view_of_the_same_bodies_uploads_nothing_and_draws_them_where_it_sees_them() {
        on_lavapipe(
            "a_new_view_of_the_same_bodies_uploads_nothing_and_draws_them_where_it_sees_them",
            |adapter| {
                let (device, queue) = device(adapter).unwrap();
                let mut r = Renderer::new(adapter, device, queue, OUTPUT_FORMAT).unwrap();
                // The bench's box from the front: its front face (Face3) at the centre, six
                // faces uploaded.
                let front = front_of_a_box(64, 48);
                assert_eq!(centre_id(&mut r, &front), [1, 3]);
                assert_eq!(r.uploads(), 6);
                // From above, up +y: its top (Face6) at the centre, from the same meshes.
                let mut top = front.clone();
                top.camera = Camera {
                    eye: [21.0, 10.0, 60.0],
                    up: [0.0, 1.0, 0.0],
                    ..front.camera
                };
                assert_eq!(centre_id(&mut r, &top), [1, 6]);
                assert_eq!(r.uploads(), 6);
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
                assert_eq!(centre_id(&mut r, &right), [1, 2]);
                assert_eq!(r.uploads(), 6);
                // The front again draws the first render's ID buffer.
                let first = r.render(&front).unwrap().ids;
                assert_eq!(r.uploads(), 6);
                assert_eq!(r.render(&front).unwrap().ids, first);
                // A highlight uploads the face's tinted mesh once, and no view after it does.
                let mut lit = front.clone();
                lit.bodies[0].highlight_faces = vec![2];
                assert_eq!(centre_id(&mut r, &lit), [1, 3]);
                assert_eq!(r.uploads(), 7);
                lit.camera = top.camera;
                assert_eq!(centre_id(&mut r, &lit), [1, 6]);
                assert_eq!(r.uploads(), 7);
                // A body whose mesh changed is uploaded again; one beside it is new.
                let mut moved = front.clone();
                moved.bodies[0].mesh = box_mesh([1.0, 0.0, 0.0], [40.0, 20.0, 10.0]);
                let mut beside = moved.bodies[0].clone();
                beside.body = 2;
                beside.mesh = box_mesh([100.0, 0.0, 0.0], [1.0; 3]);
                moved.bodies.push(beside);
                assert_eq!(centre_id(&mut r, &moved), [1, 3]);
                assert_eq!(r.uploads(), 19);
                // Only the last scene's bodies are kept.
                assert_eq!(r.bodies.keys().copied().collect::<Vec<_>>(), [1, 2]);
                assert_eq!(centre_id(&mut r, &front), [1, 3]);
                assert_eq!(r.uploads(), 25);
                assert_eq!(r.bodies.keys().copied().collect::<Vec<_>>(), [1]);
                // A face highlighted the first time it's drawn is uploaded tinted alone.
                let mut lit = moved;
                lit.bodies.truncate(1);
                lit.bodies[0].highlight_faces = vec![2];
                assert_eq!(centre_id(&mut r, &lit), [1, 3]);
                assert_eq!(r.uploads(), 31);
            },
        );
    }
}
