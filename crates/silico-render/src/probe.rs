//! M4.6's first step, proved: a 40 × 20 × 10 box drawn headless, each face an instance of its
//! own (its picking instance the face's number, as OCCT numbers a box's faces), from the front,
//! off the image's centre so a flipped or mirrored image shows, and its ID buffer read back
//! and checked against the front face's rectangle projected by hand (docs/design/m4.6-render.md,
//! Build steps, 1).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use re_renderer::mesh::{CpuMesh, GpuMesh, Material};
use re_renderer::renderer::{GpuMeshInstance, MeshDrawData};
use re_renderer::view_builder::{
    OrthographicCameraMode, Projection, TargetConfiguration, ViewBuilder, ViewBuilderId,
    ViewPickingConfiguration,
};
use re_renderer::{
    PickingLayerId, PickingLayerInstanceId, PickingLayerObjectId, PickingLayerProcessor, RectInt,
    RenderConfig, RenderContext, Rgba,
};

/// What a probe render found.
#[derive(Debug)]
pub struct Probed {
    /// The adapter it rendered on.
    pub adapter: String,
    /// Pixels by (body, face) in the ID buffer; (0, 0) is the background.
    pub ids: BTreeMap<(u64, u64), u64>,
    /// The first and last columns and rows (from the top) the front face covers, if any.
    pub front: Option<[u32; 4]>,
    /// Pixels more than one pixel inside the front face's hand-projected rectangle that
    /// aren't the front face, or more than one outside it that aren't the background.
    pub off: u64,
    /// How long making the render context took.
    pub context: Duration,
    /// How long the render and its readback took.
    pub render: Duration,
}

/// Where the box's front face lands in the view, about its centre: x from −19 to 21 mm, z
/// from −3 to 7 mm, so it's off the image's centre both ways, and every edge in view.
pub const FRONT: [f64; 4] = [-19.0, 21.0, -3.0, 7.0];
/// The point of the box drawn at the view's centre: its x and z, 19 and 3, put the front
/// face at [`FRONT`] (its y, along the view, doesn't move it).
const CENTRE: [f32; 3] = [19., 10., 3.];
/// The view's height, 1.1 × the box's 40 mm width.
pub const VIEW_HEIGHT: f32 = 44.0;
/// Frames to wait for the picking readback: re_renderer delivers it in a later frame, and
/// each frame here waits for the GPU, so it comes in the first or second. The bound is for
/// a driver that never delivers it.
const READBACK_FRAMES: u32 = 100;

/// The box's faces, in OCCT's order for a box from the origin: x = 0, x = 40, y = 0, y = 20,
/// z = 0, z = 10, each its outward normal and its corners counterclockwise from outside.
const FACES: [([f32; 3], [[f32; 3]; 4]); 6] = [
    (
        [-1., 0., 0.],
        [[0., 0., 0.], [0., 0., 10.], [0., 20., 10.], [0., 20., 0.]],
    ),
    (
        [1., 0., 0.],
        [
            [40., 0., 0.],
            [40., 20., 0.],
            [40., 20., 10.],
            [40., 0., 10.],
        ],
    ),
    (
        [0., -1., 0.],
        [[0., 0., 0.], [40., 0., 0.], [40., 0., 10.], [0., 0., 10.]],
    ),
    (
        [0., 1., 0.],
        [
            [0., 20., 0.],
            [0., 20., 10.],
            [40., 20., 10.],
            [40., 20., 0.],
        ],
    ),
    (
        [0., 0., -1.],
        [[0., 0., 0.], [0., 20., 0.], [40., 20., 0.], [40., 0., 0.]],
    ),
    (
        [0., 0., 1.],
        [
            [0., 0., 10.],
            [40., 0., 10.],
            [40., 20., 10.],
            [0., 20., 10.],
        ],
    ),
];

/// The box from the front (from −Y, +Z up) on `adapter`, `size` pixels square, its view
/// [`VIEW_HEIGHT`] high, with its front face at [`FRONT`]. `while_held` runs after the
/// render, before the render context is dropped (to measure its memory).
pub fn front_of_a_box<T>(
    adapter: &wgpu::Adapter,
    size: u32,
    while_held: impl FnOnce() -> T,
) -> Result<(Probed, T), String> {
    let started = Instant::now();
    let caps = re_renderer::device_caps::DeviceCaps::from_adapter(adapter)
        .map_err(|e| format!("the adapter can't be used: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&caps.device_descriptor()))
        .map_err(|e| format!("no device: {e}"))?;
    let mut ctx = RenderContext::new(
        adapter,
        device,
        queue,
        wgpu::TextureFormat::Rgba8Unorm,
        RenderConfig::best_for_device_caps,
    )
    .map_err(|e| format!("no render context: {e}"))?;
    let context = started.elapsed();
    ctx.begin_frame();

    let centre = glam::Vec3::from(CENTRE);
    let white = ctx.texture_manager_2d.white_texture_unorm_handle().clone();
    let mut instances = Vec::new();
    for (i, (normal, corners)) in FACES.iter().enumerate() {
        let positions: Vec<glam::Vec3> = corners
            .iter()
            .map(|p| glam::Vec3::from(*p) - centre)
            .collect();
        let mesh = CpuMesh {
            label: format!("Face{}", i + 1).into(),
            triangle_indices: vec![glam::UVec3::new(0, 1, 2), glam::UVec3::new(0, 2, 3)],
            vertex_colors: vec![
                re_renderer::Rgba32Unmul::from_rgba_unmul_array([
                    170, 175, 182, 255
                ]);
                4
            ],
            vertex_normals: vec![glam::Vec3::from(*normal); 4],
            vertex_texcoords: vec![glam::Vec2::ZERO; 4],
            bbox: macaw::BoundingBox::from_points(positions.iter().copied()),
            vertex_positions: positions,
            materials: smallvec::smallvec![Material {
                label: "body".into(),
                index_range: re_renderer::Span { start: 0, len: 6 },
                albedo: white.clone(),
                albedo_factor: Rgba::WHITE,
            }],
        };
        let gpu = GpuMesh::new(&ctx, &mesh).map_err(|e| format!("Face{}: {e}", i + 1))?;
        instances.push(GpuMeshInstance {
            gpu_mesh: Arc::new(gpu),
            world_from_mesh: glam::Affine3A::IDENTITY,
            additive_tint: re_renderer::Color32::TRANSPARENT,
            outline_mask_ids: Default::default(),
            picking_layer_id: PickingLayerId {
                object: PickingLayerObjectId(1),
                instance: PickingLayerInstanceId(i as u64 + 1),
            },
            cull_mode: None,
        });
    }
    let view_from_world = macaw::IsoTransform::look_at_rh(
        glam::Vec3::new(0., -50., 0.),
        glam::Vec3::ZERO,
        glam::Vec3::Z,
    )
    .ok_or("the camera's frame is degenerate")?;
    let mut view = ViewBuilder::new(
        &ctx,
        TargetConfiguration {
            name: "probe".into(),
            resolution_in_pixel: [size, size],
            view_from_world,
            projection_from_view: Projection::Orthographic {
                camera_mode: OrthographicCameraMode::NearPlaneCenter,
                vertical_world_size: VIEW_HEIGHT,
                far_plane_distance: 1000.0,
            },
            picking_config: Some(ViewPickingConfiguration {
                picking_rect: RectInt {
                    min: glam::IVec2::ZERO,
                    extent: glam::UVec2::new(size, size),
                },
                readback_identifier: 1,
                show_debug_view: false,
            }),
            ..Default::default()
        },
        ViewBuilderId::new(1),
    )
    .map_err(|e| format!("no view: {e}"))?;
    let draw = MeshDrawData::new(&ctx, &instances).map_err(|e| format!("no draw data: {e}"))?;
    view.queue_draw(&ctx, draw)
        .map_err(|e| format!("couldn't queue the box: {e}"))?;
    let rendering = Instant::now();
    let commands = view
        .draw(&ctx, Rgba::WHITE)
        .map_err(|e| format!("couldn't draw: {e}"))?;
    ctx.before_submit();
    ctx.queue.submit([commands]);
    let wait = || wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(std::time::Duration::from_secs(30)),
    };
    ctx.device
        .poll(wait())
        .map_err(|e| format!("the GPU didn't finish: {e}"))?;
    let mut picked = None;
    for _ in 0..READBACK_FRAMES {
        ctx.begin_frame();
        ctx.device
            .poll(wait())
            .map_err(|e| format!("the GPU didn't finish: {e}"))?;
        if let Some(result) = PickingLayerProcessor::readback_result(&ctx, 1) {
            picked = Some(result);
            break;
        }
        ctx.before_submit();
    }
    let picked = picked.ok_or("the picking layer was never read back")?;
    let render = rendering.elapsed();
    let held = while_held();

    let mut ids = BTreeMap::new();
    for id in &picked.picking_id_data {
        *ids.entry((id.object.0, id.instance.0)).or_insert(0u64) += 1;
    }
    // The front face (Face3, y = 0) projects to [`FRONT`] about the centre; the image spans
    // VIEW_HEIGHT, rows from the top (so z up is rows up).
    let pixel = f64::from(size) / f64::from(VIEW_HEIGHT);
    let half = f64::from(size) / 2.0;
    let [left, right, bottom, top] = FRONT;
    let (x0, x1) = (half + left * pixel, half + right * pixel);
    let (y0, y1) = (half - top * pixel, half - bottom * pixel);
    let mut off = 0;
    let mut front: Option<[u32; 4]> = None;
    for row in 0..size {
        for col in 0..size {
            let (cx, cy) = (f64::from(col) + 0.5, f64::from(row) + 0.5);
            let Some(id) = picked.picking_id_data.get((row * size + col) as usize) else {
                return Err("the picking layer is smaller than the image".to_owned());
            };
            let inside = cx > x0 + 1.0 && cx < x1 - 1.0 && cy > y0 + 1.0 && cy < y1 - 1.0;
            let outside = cx < x0 - 1.0 || cx > x1 + 1.0 || cy < y0 - 1.0 || cy > y1 + 1.0;
            let face = (id.object.0, id.instance.0);
            if face == (1, 3) {
                let b = front.get_or_insert([col, col, row, row]);
                *b = [b[0].min(col), b[1].max(col), b[2].min(row), b[3].max(row)];
            }
            if (inside && face != (1, 3)) || (outside && face != (0, 0)) {
                off += 1;
            }
        }
    }
    Ok((
        Probed {
            adapter: adapter.get_info().name,
            ids,
            front,
            off,
            context,
            render,
        },
        held,
    ))
}
