//! A frame's planes ([`Frame::planes`](super::Frame::planes)): flat panels drawn among the
//! bodies, translucent, and in no picking: the app's origin planes, as the approved
//! prototype draws them (docs/design/m5-ui/js/viewport.js, `buildPlanes`: a fill of
//! `MeshBasicMaterial`, transparent, double-sided, writing no depth, and an outline).
//!
//! **The fill** is a unit square of two triangles, uploaded once for each fill colour and
//! placed for each plane by its instance's transform, as a body's faces are ([`Meshes`]):
//! unlit (its normals zero, so `re_renderer` shades it with its colour alone, as a basic
//! material is), its colour's alpha in its material, so `re_renderer` draws it in its
//! transparent phase: after every opaque face, tested against
//! their depth and writing none, double-sided, premultiplied over what's there. A body in
//! front of a plane hides it; a plane in front of a body tints it, and hides nothing. Drawn
//! after the edges and lines (re_renderer draws its lines first among what's transparent),
//! so it tints an edge behind it. An edge in front of it lies on the faces it bounds, whose
//! depth hides the fill there, so it isn't tinted, but along a silhouette, the half of its
//! stroke beyond its faces; nor is a line clear of every face (a sketch's in front of the
//! plane, or on it), which the fill tints.
//!
//! **The outline** is a closed line strip, placed and moved toward the eye as a frame's
//! lines are ([`super::lines_batch`]), in its colour with its alpha, alpha blended (writing
//! no depth), at the edges' depth offset. Its fill, drawn after it, tints the half of its
//! stroke inside the plane.
//!
//! **Neither goes into the picking layer.** `re_renderer` draws every mesh and every line
//! into it, with depth, whatever its picking ID: a plane there would cover the faces and
//! edges behind it from a pick, as a sketch's lines cover what's under their stroke. So each
//! is a draw data of `re_renderer`'s own wrapped in [`Unpicked`], whose renderer draws each
//! phase its draw data asks for but the picking layer's, which it passes over: the picking
//! layer of a frame with planes is the frame's without them, pixel for pixel, and a plane
//! takes no click.

use std::sync::Arc;

use re_renderer::mesh::{CpuMesh, GpuMesh, Material};
use re_renderer::renderer::{
    DrawData, DrawError, DrawInstruction, DrawableCollectionViewInfo, GpuMeshInstance,
    LineDrawData, MeshDrawData, Renderer as Draws,
};
use re_renderer::{
    Color32, DrawPhase, DrawableCollector, GpuRenderPipelinePoolAccessor, LineDrawableBuilder,
    PickingLayerInstanceId, PickingLayerObjectId, RenderContext, Rgba, Rgba32Unmul, Size,
};
use silico_protocol::vec3::{cross, dot};
use silico_protocol::{Class, InvalidParameter, MAX_RENDER_COORDINATE};

use super::{
    EDGE_DEPTH_OFFSET, MAX_LINE_WIDTH, Strips, View, add_edge_strips, drawn_stroke_fraction, vec3,
};

/// The most planes a frame may draw: each is placed and drawn on its own, each frame, on the
/// caller's thread (a transparent mesh isn't batched).
pub const MAX_FRAME_PLANES: usize = 64;

/// A flat four-sided panel a frame draws among its bodies, translucent, and in no picking
/// ([`Frame::planes`](super::Frame::planes)): the parallelogram from `origin` along `u` and
/// `v`, its corners `origin`, `origin + u`, `origin + u + v`, and `origin + v`, in the
/// document's coordinates, each finite and within [`MAX_RENDER_COORDINATE`] of the origin.
/// Filled in `fill` and outlined in `outline`, each sRGB with its alpha unmultiplied (the
/// fill's below 255: a plane is translucent, so what's behind it shows, as it's picked), the
/// outline `width` points wide (above 0, at most [`MAX_LINE_WIDTH`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub origin: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
    pub fill: [u8; 4],
    pub outline: [u8; 4],
    pub width: f32,
}

impl Plane {
    /// Its corners in order around it: `origin`, `origin + u`, `origin + u + v`, and
    /// `origin + v`, each in f64.
    pub fn corners(&self) -> [[f64; 3]; 4] {
        let at = |s: f64, t: f64| -> [f64; 3] {
            std::array::from_fn(|k| self.origin[k] + s * self.u[k] + t * self.v[k])
        };
        [at(0.0, 0.0), at(1.0, 0.0), at(1.0, 1.0), at(0.0, 1.0)]
    }

    /// Its outline, one closed strip of its corners and the first again; none for a plane
    /// [`check`] refuses.
    pub(super) fn outline(&self) -> Option<Strips> {
        let [a, b, c, d] = self.corners();
        Strips::new([vec![a, b, c, d, a]]).ok()
    }
}

/// Why a frame can't draw `planes`, if it can't, naming `planes` and which: more than
/// [`MAX_FRAME_PLANES`], a corner not finite or past [`MAX_RENDER_COORDINATE`], a fill that
/// isn't translucent, or a width not above 0 or past [`MAX_LINE_WIDTH`].
pub(super) fn check(planes: &[Plane]) -> Result<(), InvalidParameter> {
    let refused = |message: String| InvalidParameter {
        parameter: "planes",
        message,
    };
    if planes.len() > MAX_FRAME_PLANES {
        return Err(refused(format!(
            "{} planes, more than a frame's {MAX_FRAME_PLANES}",
            planes.len()
        )));
    }
    for (i, plane) in planes.iter().enumerate() {
        let within = |p: &[f64; 3]| p.iter().all(|c| c.abs() <= MAX_RENDER_COORDINATE);
        if let Some((k, corner)) = plane.corners().iter().enumerate().find(|(_, p)| !within(p)) {
            return Err(refused(format!(
                "planes[{i}]'s corner {k}, {corner:?}, must be finite and within {MAX_RENDER_COORDINATE} mm of the origin"
            )));
        }
        if plane.fill[3] == 255 {
            return Err(refused(format!(
                "planes[{i}]'s fill {:?} must be translucent, its alpha below 255: what's behind a plane shows, as it's picked",
                plane.fill
            )));
        }
        let width = plane.width;
        if !(width.is_finite() && width > 0.0 && width <= MAX_LINE_WIDTH) {
            return Err(refused(format!(
                "planes[{i}]'s width {width:?}, in points, must be above 0 and at most {MAX_LINE_WIDTH}"
            )));
        }
    }
    Ok(())
}

/// The most fill meshes kept ([`Meshes`]): one for each fill colour drawn, so a frame's few
/// colours (the app's two) are uploaded once; past it, every one kept is let go and those
/// drawn uploaded again.
pub(super) const MAX_FILL_MESHES: usize = 16;

/// The planes' fill meshes, by fill colour: each the unit square (corners (0, 0, 0),
/// (1, 0, 0), (1, 1, 0), (0, 1, 0)) in that colour, uploaded the first time a frame draws a
/// plane of it, and placed for each plane by its instance's transform ([`fill`]), as a body's
/// faces are: so a frame of the same colours uploads nothing.
pub(super) type Meshes = std::collections::BTreeMap<[u8; 4], Arc<GpuMesh>>;

/// `planes`, each with its outline ([`Plane::outline`]), drawn in `view`: their fills, an
/// instance each of its colour's mesh in `meshes` (uploaded if it has none), and their
/// outlines, a batch each, as the module says, each [`Unpicked`]; none for a plane whose
/// outline's stroke can't reach the image. How many were drawn, and the draw data to queue:
/// the fills', if any, and the outlines', if any.
pub(super) fn draw(
    ctx: &RenderContext,
    meshes: &mut Meshes,
    planes: &[(Plane, Strips)],
    view: &View,
    pixels_per_point: f32,
) -> Result<(usize, Vec<re_renderer::QueueableDrawData>), String> {
    let mut fills = Vec::new();
    let mut outlines = LineDrawableBuilder::new(ctx);
    outlines.enable_alpha_blending();
    let mut drawn = 0usize;
    for (i, (plane, outline)) in planes.iter().enumerate() {
        let Some(bounds) = outline.bounds() else {
            continue;
        };
        let stroke = view.stroke_frustum(plane.width, pixels_per_point);
        if stroke.class_of_box(bounds) == Class::Out {
            continue;
        }
        drawn += 1;
        if plane.fill[3] > 0 {
            fills.push(fill(ctx, meshes, plane, view)?);
        }
        if plane.outline[3] > 0 {
            let mut batch = outlines
                .batch(format!("Plane{i}"))
                .world_from_obj(glam::Affine3A::IDENTITY)
                .picking_object_id(PickingLayerObjectId(0))
                .depth_offset(EDGE_DEPTH_OFFSET);
            let [red, green, blue, alpha] = plane.outline;
            let style = |strip: re_renderer::LineStripBuilder<'_, '_>| {
                strip
                    .radius(Size::new_ui_points(plane.width / 2.0))
                    .color(Color32::from_rgba_unmultiplied(red, green, blue, alpha))
                    .picking_instance_id(PickingLayerInstanceId(0));
            };
            let reach = drawn_stroke_fraction(view.height, plane.width, pixels_per_point);
            let put = |p: [f64; 3]| view.strip_from_camera(p, reach);
            for strip in outline.strips() {
                add_edge_strips(&mut batch, view, &stroke, strip, put, style);
            }
        }
    }
    let mut queued = Vec::new();
    if !fills.is_empty() {
        let draw = MeshDrawData::new(ctx, &fills)
            .map_err(|e| format!("no draw data for the planes' fills: {e}"))?;
        queued.push(Unpicked(draw).into());
    }
    if !outlines.is_empty() {
        let draw = outlines
            .into_draw_data()
            .map_err(|e| format!("no draw data for the planes' outlines: {e}"))?;
        queued.push(Unpicked(draw).into());
    }
    Ok((drawn, queued))
}

/// A plane's fill as an instance of its colour's unit square ([`Meshes`], uploaded here if
/// `meshes` has none), placed by its transform: the plane's `u`, `v`, and their unit normal
/// rotated into the view, and its origin about the camera, each in f64 and then f32
/// ([`View::about_camera`], as a body's faces and edges are placed), so the square's corners
/// are the plane's. Unlit (normals zero), its alpha the material's (premultiplied: the colour
/// in linear light times the alpha, so `re_renderer`'s premultiplied blend takes it as it is),
/// drawn from both sides.
fn fill(
    ctx: &RenderContext,
    meshes: &mut Meshes,
    plane: &Plane,
    view: &View,
) -> Result<GpuMeshInstance, String> {
    let mesh = match meshes.get(&plane.fill) {
        Some(mesh) => Arc::clone(mesh),
        None => {
            if meshes.len() >= MAX_FILL_MESHES {
                meshes.clear();
            }
            let mesh = Arc::new(unit_square(ctx, plane.fill)?);
            meshes.insert(plane.fill, Arc::clone(&mesh));
            mesh
        }
    };
    let rotated = |d: [f64; 3]| {
        let f = &view.frame;
        [dot(f.right, d), dot(f.up, d), dot(f.back, d)]
    };
    let (u, v) = (rotated(plane.u), rotated(plane.v));
    let n = cross(u, v);
    let length = dot(n, n).sqrt();
    let normal = if length > 0.0 {
        n.map(|c| c / length)
    } else {
        [0.0; 3]
    };
    let mut instance = GpuMeshInstance::new(mesh);
    instance.world_from_mesh = glam::Affine3A::from_cols(
        vec3(u).into(),
        vec3(v).into(),
        vec3(normal).into(),
        vec3(view.about_camera(plane.origin)).into(),
    );
    // Both sides, as the prototype's `DoubleSide`: GpuMeshInstance::new's, said here.
    instance.cull_mode = None;
    Ok(instance)
}

/// The unit square in `fill` (sRGB, its alpha unmultiplied), as a GPU mesh.
fn unit_square(ctx: &RenderContext, fill: [u8; 4]) -> Result<GpuMesh, String> {
    let _span = tracing::debug_span!("plane_mesh", fill = ?fill).entered();
    let positions = vec![
        glam::Vec3::ZERO,
        glam::Vec3::X,
        glam::Vec3::new(1.0, 1.0, 0.0),
        glam::Vec3::Y,
    ];
    let [red, green, blue, alpha] = fill;
    let a = f32::from(alpha) / 255.0;
    let cpu = CpuMesh {
        label: "Plane".into(),
        materials: smallvec::smallvec![Material {
            label: "plane".into(),
            index_range: re_renderer::Span { start: 0, len: 6 },
            albedo: ctx.texture_manager_2d.white_texture_unorm_handle().clone(),
            albedo_factor: Rgba::from_rgba_premultiplied(a, a, a, a),
        }],
        triangle_indices: vec![glam::UVec3::new(0, 1, 2), glam::UVec3::new(0, 2, 3)],
        vertex_colors: vec![Rgba32Unmul::from_rgba_unmul_array([red, green, blue, 255]); 4],
        vertex_normals: vec![glam::Vec3::ZERO; 4],
        vertex_texcoords: vec![glam::Vec2::ZERO; 4],
        bbox: macaw::BoundingBox::from_points(positions.iter().copied()),
        vertex_positions: positions,
    };
    GpuMesh::new(ctx, &cpu).map_err(|e| format!("a plane's mesh: {e}"))
}

/// The renderers [`Unpicked`] draw data needs, registered with `ctx`'s: a context draws
/// only through renderers it knows.
pub(super) fn register(ctx: &mut RenderContext) {
    let renderers = ctx.renderers_mut();
    renderers.register::<UnpickedRenderer<<MeshDrawData as DrawData>::Renderer>>();
    renderers.register::<UnpickedRenderer<<LineDrawData as DrawData>::Renderer>>();
}

/// A draw data of `re_renderer`'s drawn in every phase it asks for but the picking layer's:
/// it collects its drawables as it would (its picking ones too), and its renderer
/// ([`UnpickedRenderer`]) draws nothing for the picking layer. `re_renderer` has no option
/// to leave a mesh or a line out of it.
pub(super) struct Unpicked<D>(pub(super) D);

impl<D> DrawData for Unpicked<D>
where
    D: DrawData + Send + Sync + 'static,
    D::Renderer: 'static,
{
    type Renderer = UnpickedRenderer<D::Renderer>;

    fn collect_drawables(
        &self,
        view_info: &DrawableCollectionViewInfo,
        collector: &mut DrawableCollector<'_>,
    ) {
        self.0.collect_drawables(view_info, collector);
    }
}

/// The renderer of an [`Unpicked`] draw data: its own draw data's renderer, which draws it
/// in every phase but the picking layer's.
pub(super) struct UnpickedRenderer<R>(R);

impl<R> Draws for UnpickedRenderer<R>
where
    R: Draws + Send + Sync + 'static,
    R::RendererDrawData: DrawData<Renderer = R> + Send + Sync,
{
    type RendererDrawData = Unpicked<R::RendererDrawData>;

    fn create_renderer(ctx: &RenderContext) -> Self {
        UnpickedRenderer(R::create_renderer(ctx))
    }

    fn draw(
        &self,
        render_pipelines: &GpuRenderPipelinePoolAccessor<'_>,
        phase: DrawPhase,
        pass: &mut wgpu::RenderPass<'_>,
        draw_instructions: &[DrawInstruction<'_, Self::RendererDrawData>],
    ) -> Result<(), DrawError> {
        if phase == DrawPhase::PickingLayer {
            return Ok(());
        }
        let inner: Vec<DrawInstruction<'_, R::RendererDrawData>> = draw_instructions
            .iter()
            .map(|instruction| DrawInstruction {
                draw_data: &instruction.draw_data.0,
                drawables: instruction.drawables,
            })
            .collect();
        self.0.draw(render_pipelines, phase, pass, &inner)
    }
}
