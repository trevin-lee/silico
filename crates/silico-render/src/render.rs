//! A [`RenderScene`] drawn headless (docs/design/m4.6-render.md): each body's faces shaded
//! by a key light and a fill fixed to the camera, as Fusion's are, its edges over them, its highlights in Fusion's selection blue, read back
//! as an 8-bit sRGB PNG and, when asked, the ID buffer. Visuals follow Fusion 360 (the
//! design's *What a render looks like*).

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
    RenderContext, Rgba, ScreenshotProcessor, Size,
};
use silico_protocol::{Camera, EDGE_BIT, PixelId, Projection, RenderBody, RenderScene};

/// The background: a flat light grey, sRGB.
pub const BACKGROUND: [u8; 3] = [230, 233, 237];
/// Fusion's default appearance (steel, satin), sRGB.
pub const BODY: [u8; 3] = [170, 175, 182];
/// Edges, sRGB.
pub const EDGE: [u8; 3] = [40, 40, 40];
/// Fusion's selection blue, sRGB.
pub const HIGHLIGHT: [u8; 3] = [6, 150, 215];
/// The light a face facing away from every light still gets, of its full albedo.
pub const AMBIENT: f32 = 0.30;
/// The key light's share, from above and to the left of the camera (Fusion's look: a box's
/// faces seen at a corner each in a shade of its own, the top lightest).
pub const KEY: f32 = 0.65;
/// The fill light's share, from below and to the right, softer.
pub const FILL: f32 = 0.20;
/// How much of the highlight's blue a highlighted face is tinted with.
pub const HIGHLIGHT_STRENGTH: f32 = 0.5;
/// Edges' widths in pixels: every edge, dark and solid, and a highlighted one.
pub const EDGE_WIDTH: f32 = 2.0;
pub const HIGHLIGHTED_EDGE_WIDTH: f32 = 4.0;
/// A highlighted face's outline, in pixels.
pub const OUTLINE_WIDTH: f32 = 1.5;
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

/// A render context on one adapter, kept between renders.
pub struct Renderer {
    ctx: RenderContext,
    /// The next render's readbacks' identifiers (each render takes two), so a render that
    /// failed never leaves a readback a later one could take as its own, for a caller that
    /// keeps the renderer after a failure (the worker doesn't; the bench could).
    readback: u64,
}

impl Renderer {
    /// A render context on `adapter`.
    pub fn new(adapter: &wgpu::Adapter) -> Result<Renderer, String> {
        let caps = re_renderer::device_caps::DeviceCaps::from_adapter(adapter)
            .map_err(|e| format!("the adapter can't be used: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&caps.device_descriptor()))
            .map_err(|e| format!("no device: {e}"))?;
        let ctx = RenderContext::new(
            adapter,
            device,
            queue,
            wgpu::TextureFormat::Rgba8Unorm,
            RenderConfig::best_for_device_caps,
        )
        .map_err(|e| format!("no render context: {e}"))?;
        Ok(Renderer { ctx, readback: 1 })
    }

    /// `scene` drawn. The scene must have passed [`RenderScene::check`]. Errors the GPU
    /// reports while it draws (out of memory, or anything wgpu finds invalid) fail the
    /// render rather than leave a blank image.
    pub fn render(&mut self, scene: &RenderScene) -> Result<Rendered, RenderFailure> {
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
        let ctx = &mut self.ctx;
        ctx.begin_frame();
        let camera = &scene.camera;
        let view = View::of(camera, scene)?;
        let mut instances = Vec::new();
        let mut lines = LineDrawableBuilder::new(ctx);
        lines.enable_alpha_blending();
        let highlighted = scene
            .bodies
            .iter()
            .any(|b| !b.highlight_faces.is_empty() || !b.highlight_edges.is_empty());
        for body in &scene.bodies {
            faces(ctx, body, &view, &mut instances)?;
            edges(&mut lines, body, &view)?;
        }
        let config = TargetConfiguration {
            name: "render".into(),
            resolution_in_pixel: [scene.width, scene.height],
            view_from_world: view.view_from_world,
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

/// The camera as re_renderer takes it. Positions are drawn about a reference point,
/// subtracted in f64 before they're f32: the target for an orthographic view, the eye for a
/// perspective one, so what's drawn keeps its precision however far it is from the origin.
struct View {
    view_from_world: macaw::IsoTransform,
    projection: ViewProjection,
    reference: [f64; 3],
    /// The key light's and the fill light's directions, toward them: fixed to the camera,
    /// as Fusion's are, the same at every point.
    key: [f64; 3],
    fill: [f64; 3],
}

/// The lights' directions in the camera's frame: the key `back + up/2 − right/2` (above and
/// to the left), the fill `back − 3up/10 + 3right/5` (below and to the right), each of unit
/// length.
fn lights(frame: &silico_protocol::Frame) -> ([f64; 3], [f64; 3]) {
    let toward = |u: f64, r: f64| {
        let v: [f64; 3] =
            std::array::from_fn(|k| frame.back[k] + u * frame.up[k] + r * frame.right[k]);
        let l = dot(v, v).sqrt();
        v.map(|c| c / l)
    };
    (toward(0.5, -0.5), toward(-0.3, 0.6))
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// In f32 on purpose: re_renderer draws in f32, after the reference is subtracted in f64.
fn vec3(a: [f64; 3]) -> glam::Vec3 {
    glam::Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32)
}

impl View {
    fn of(camera: &Camera, scene: &RenderScene) -> Result<View, String> {
        let frame = camera.frame().map_err(|e| e.message)?;
        let (back, up) = (vec3(frame.back), vec3(frame.up));
        let (key, fill) = lights(&frame);
        let view = match camera.projection {
            Projection::Orthographic { view_height } => {
                // Only the eye's direction counts: the camera is set past every point of the
                // scene, so nothing is cut off behind the eye (FreeCAD fits its clipping
                // planes to the scene, too), and the far plane past the scene's other side.
                let reach = scene.reach(camera.target);
                let margin = (reach * CLEARANCE).max(1.0);
                let at = back * (reach + margin) as f32;
                View {
                    view_from_world: macaw::IsoTransform::look_at_rh(at, glam::Vec3::ZERO, up)
                        .ok_or("the camera's frame is degenerate")?,
                    projection: ViewProjection::Orthographic {
                        camera_mode: OrthographicCameraMode::NearPlaneCenter,
                        vertical_world_size: view_height as f32,
                        far_plane_distance: (2.0 * (reach + margin)) as f32,
                    },
                    reference: camera.target,
                    key,
                    fill,
                }
            }
            Projection::Perspective { field_of_view } => {
                // The near plane from what's in view, not from where the target is along
                // the ray: a millionth of the farthest point's distance (reverse-z keeps the
                // depth precise from there out).
                let reach = scene.reach(camera.eye).max(frame.distance);
                View {
                    view_from_world: macaw::IsoTransform::look_at_rh(glam::Vec3::ZERO, -back, up)
                        .ok_or("the camera's frame is degenerate")?,
                    projection: ViewProjection::Perspective {
                        vertical_fov: field_of_view.to_radians() as f32,
                        near_plane_distance: (reach * NEAR_PLANE_OF_REACH) as f32,
                        aspect_ratio: scene.width as f32 / scene.height as f32,
                    },
                    reference: camera.eye,
                    key,
                    fill,
                }
            }
        };
        Ok(view)
    }

    /// A position about the reference, as drawn.
    fn place(&self, p: [f64; 3]) -> glam::Vec3 {
        vec3(sub(p, self.reference))
    }

    /// A vertex's light, from its normal: the ambient, and the key's and the fill's shares
    /// by Lambert's law, at most the full albedo.
    fn light(&self, normal: [f32; 3]) -> f32 {
        let n = normal.map(f64::from);
        let lambert = |toward: [f64; 3]| dot(n, toward).clamp(0.0, 1.0) as f32;
        (AMBIENT + KEY * lambert(self.key) + FILL * lambert(self.fill)).min(1.0)
    }
}

/// An sRGB colour as re_renderer's linear one.
fn srgb([r, g, b]: [u8; 3]) -> Rgba {
    Rgba::from_srgba_unmultiplied(r, g, b, 255)
}

/// Each face of `body` as a mesh instance of its own, its picking ID the body's number and
/// the face's from 1. Its lighting is in its vertex colours, its normals zero, so
/// re_renderer draws it unlit (the camera's own lights, which re_renderer's lighting isn't).
fn faces(
    ctx: &RenderContext,
    body: &RenderBody,
    view: &View,
    instances: &mut Vec<GpuMeshInstance>,
) -> Result<(), String> {
    let mesh = &body.mesh;
    let albedo = srgb(BODY);
    let blue = srgb(HIGHLIGHT);
    let white = ctx.texture_manager_2d.white_texture_unorm_handle().clone();
    let mut first = 0usize;
    for (face, &count) in mesh.face_triangles.iter().enumerate() {
        let count = usize::try_from(count).map_err(|e| e.to_string())?;
        let triangles = &mesh.triangles[first..first + count];
        first += count;
        if triangles.is_empty() {
            continue;
        }
        let number = u32::try_from(face + 1).map_err(|e| e.to_string())?;
        let highlighted = body.highlight_faces.binary_search(&(number - 1)).is_ok();
        // The face's own vertices, renumbered from 0.
        let mut index = std::collections::BTreeMap::new();
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        let mut renumbered = Vec::with_capacity(triangles.len());
        for t in triangles {
            let mut out = [0u32; 3];
            for (k, &v) in t.iter().enumerate() {
                let next = u32::try_from(index.len()).map_err(|e| e.to_string())?;
                let i = usize::try_from(v).map_err(|e| e.to_string())?;
                out[k] = *index.entry(v).or_insert_with(|| {
                    let p = mesh.positions[i];
                    let light = view.light(mesh.normals[i]);
                    // In linear light: the albedo scaled by the light, half blue if
                    // highlighted, then sRGB (an Rgba's alpha must stay 1, or the
                    // conversion premultiplies in sRGB).
                    let mut c = [albedo.r(), albedo.g(), albedo.b()].map(|a| a * light);
                    if highlighted {
                        let b = [blue.r(), blue.g(), blue.b()];
                        for k in 0..3 {
                            c[k] = c[k] * (1.0 - HIGHLIGHT_STRENGTH) + b[k] * HIGHLIGHT_STRENGTH;
                        }
                    }
                    let c = Rgba::from_rgb(c[0], c[1], c[2]);
                    positions.push(view.place(p));
                    let c = Color32::from(c);
                    colors.push(re_renderer::Rgba32Unmul::from_rgba_unmul_array([
                        c.r(),
                        c.g(),
                        c.b(),
                        255,
                    ]));
                    next
                });
            }
            renumbered.push(glam::UVec3::from(out));
        }
        let vertices = positions.len();
        let cpu = CpuMesh {
            label: format!("Body{}Face{number}", body.body).into(),
            triangle_indices: renumbered,
            vertex_colors: colors,
            vertex_normals: vec![glam::Vec3::ZERO; vertices],
            vertex_texcoords: vec![glam::Vec2::ZERO; vertices],
            bbox: macaw::BoundingBox::from_points(positions.iter().copied()),
            vertex_positions: positions,
            materials: smallvec::smallvec![Material {
                label: "body".into(),
                index_range: re_renderer::Span {
                    start: 0,
                    len: u32::try_from(triangles.len() * 3).map_err(|e| e.to_string())?,
                },
                albedo: white.clone(),
                albedo_factor: Rgba::WHITE,
            }],
        };
        let gpu = GpuMesh::new(ctx, &cpu).map_err(|e| format!("Face{number}: {e}"))?;
        instances.push(GpuMeshInstance {
            gpu_mesh: Arc::new(gpu),
            world_from_mesh: glam::Affine3A::IDENTITY,
            // Opaque black: re_renderer adds the tint's colour and multiplies by its alpha,
            // so this leaves the face's own colour.
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

/// Each edge of `body` as a line strip, its picking ID the body's number and the edge's
/// from 1 with [`EDGE_BIT`]; a highlighted one wider, in the highlight's blue.
fn edges(
    lines: &mut LineDrawableBuilder<'_>,
    body: &RenderBody,
    view: &View,
) -> Result<(), String> {
    let mesh = &body.mesh;
    let mut batch = lines
        .batch(format!("Body{}", body.body))
        .picking_object_id(PickingLayerObjectId(u64::from(body.body)))
        .depth_offset(EDGE_DEPTH_OFFSET);
    let mut first = 0usize;
    for (edge, &count) in mesh.edge_point_counts.iter().enumerate() {
        let count = usize::try_from(count).map_err(|e| e.to_string())?;
        let points = &mesh.edge_points[first..first + count];
        first += count;
        if points.len() < 2 {
            continue;
        }
        let number = u32::try_from(edge + 1).map_err(|e| e.to_string())?;
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
    Ok(())
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
