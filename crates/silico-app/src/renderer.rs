//! The viewport's renderer, as the pane sees it (docs/design/m5.0-app.md, section 10; M5.6):
//! bodies uploaded once by the pane, each frame's view (the camera, the size, the bodies to
//! draw, the highlights, and a picking rectangle), drawn into egui's render pass, and the
//! picking result a frame or two late. It's `silico-render`'s renderer, on eframe's device
//! (one renderer with agents' renders: M4.6's R1), whose frame types these are; it places
//! every face and edge itself from the camera, in `f64` (section 10, *The bodies move
//! instead*), so the pane passes the camera and no transform. [`ViewRenderer`] is the
//! pane's view of it, so a test can hand the pane a renderer that needs no GPU.
//!
//! The renderer lives in egui_wgpu's `callback_resources` ([`on_eframe`], [`install`]), and
//! each frame reaches it through a paint callback ([`OnGpu`]): the pane's uploads and
//! forgets, one waiting for each body until a callback is prepared, then the frame. A test
//! hands the pane a [`Present`] of its own, and [`apply`]s the changes to a renderer.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui;
use eframe::egui_wgpu;
use eframe::wgpu;
use silico_protocol::{Camera, Mesh};
pub use silico_render::render::{
    Element, Frame, Outlines, Pick, Picking, PixelRect, RenderFailure, Renderer,
};

/// A [`Frame`] whose values it holds: what the pane hands on, to be drawn once the paint
/// callback is prepared.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameData {
    pub serial: u64,
    pub camera: Camera,
    pub size_px: [u32; 2],
    pub pixels_per_point: f32,
    pub bodies: Vec<u32>,
    pub highlights: Vec<(u32, Element)>,
    pub hovered: Vec<(u32, Element)>,
    pub outlines: Outlines,
    pub picking: Option<PixelRect>,
}

impl FrameData {
    /// The frame it holds.
    pub fn frame(&self) -> Frame<'_> {
        Frame {
            serial: self.serial,
            camera: &self.camera,
            size_px: self.size_px,
            pixels_per_point: self.pixels_per_point,
            bodies: &self.bodies,
            highlights: &self.highlights,
            hovered: &self.hovered,
            outlines: self.outlines,
            picking: self.picking,
            background: crate::theme::VIEWPORT_BG,
        }
    }
}

/// The pane's view of a renderer (M5.6): `silico_render`'s frame API, which [`Renderer`]
/// is, and a test's fake.
pub trait ViewRenderer: Send + Sync + 'static {
    /// Why it refused something.
    type Failure: std::fmt::Display;
    /// Holds `mesh` as body `body`'s, in place of any it held: the pane calls it once for
    /// each body's key and level. No GPU work until [`prepare`](Self::prepare).
    fn upload(&mut self, body: u32, mesh: &Arc<Mesh>) -> Result<(), Self::Failure>;
    /// Lets go of body `body`'s mesh: the pane calls it for a body gone.
    fn forget(&mut self, body: u32);
    /// Prepares `frame`'s drawing, once an egui frame: its bodies placed from its camera,
    /// its highlights, and its picking. A frame refused draws nothing until one succeeds.
    fn prepare(&mut self, frame: &Frame<'_>) -> Result<wgpu::CommandBuffer, Self::Failure>;
    /// Whether `failure` is a frame refused for the moment, to be tried again by a later
    /// frame (the renderer backing off after the GPU failed one), not a failure to say.
    /// While a renderer backs off the pane asks for frames at once ([`Present::backing_off`]),
    /// so a backoff counted in frames runs at the display's rate, about a second at most,
    /// not at the idle rate.
    fn retry_later(_failure: &Self::Failure) -> bool {
        false
    }
    /// Draws what [`prepare`](Self::prepare) made into egui's render pass, its viewport
    /// already the pane's.
    fn composite(&self, pass: &mut wgpu::RenderPass<'_>);
    /// The answer to the rectangle a frame asked to pick under, from the frame after it;
    /// none until then.
    fn take_picking(&mut self) -> Option<Picking>;
}

impl ViewRenderer for Renderer {
    type Failure = RenderFailure;
    fn upload(&mut self, body: u32, mesh: &Arc<Mesh>) -> Result<(), RenderFailure> {
        Renderer::upload(self, body, mesh)
    }
    fn forget(&mut self, body: u32) {
        Renderer::forget(self, body);
    }
    fn prepare(&mut self, frame: &Frame<'_>) -> Result<wgpu::CommandBuffer, RenderFailure> {
        Renderer::prepare(self, frame)
    }
    fn retry_later(failure: &RenderFailure) -> bool {
        matches!(failure, RenderFailure::BackingOff { .. })
    }
    fn composite(&self, pass: &mut wgpu::RenderPass<'_>) {
        Renderer::composite(self, pass);
    }
    fn take_picking(&mut self) -> Option<Picking> {
        Renderer::take_picking(self)
    }
}

/// The viewport's renderer made on eframe's device (its adapter, device, queue, and target
/// format), installed in its paint callbacks' resources, and the [`Present`] that reaches it;
/// or, if `silico-render` can't draw on that device, why, said in the log too: the app then
/// runs with no viewport drawn, the pane saying why ([`crate::App::not_drawing`]).
pub fn on_eframe(render_state: &egui_wgpu::RenderState) -> Result<OnGpu<Renderer>, String> {
    match Renderer::new(
        &render_state.adapter,
        render_state.device.clone(),
        render_state.queue.clone(),
        render_state.target_format,
    ) {
        Ok(renderer) => {
            install(render_state, renderer);
            Ok(OnGpu::new())
        }
        Err(why) => {
            tracing::warn!("the viewport can't draw on this device: {why}");
            Err(why)
        }
    }
}

/// A change to the bodies a renderer holds.
#[derive(Debug, Clone)]
pub enum Change {
    Upload(u32, Arc<Mesh>),
    Forget(u32),
}

/// `changes` made to `renderer`, in order; each upload it refused said in the log, and its
/// body returned, so it's drawn by no frame until uploaded again.
pub fn apply<R: ViewRenderer>(renderer: &mut R, changes: Vec<Change>) -> BTreeSet<u32> {
    let mut refused = BTreeSet::new();
    for change in changes {
        match change {
            Change::Upload(body, mesh) => match renderer.upload(body, &mesh) {
                Ok(()) => {
                    refused.remove(&body);
                }
                Err(failure) => {
                    tracing::warn!(
                        body,
                        "the viewport's renderer refused a body's mesh: {failure}"
                    );
                    refused.insert(body);
                }
            },
            Change::Forget(body) => {
                refused.remove(&body);
                renderer.forget(body);
            }
        }
    }
    refused
}

/// How the pane's frames reach a renderer.
pub trait Present {
    /// This frame's changes to the bodies held, in order, and its view, if a tab is shown,
    /// drawn in `rect`.
    fn present(
        &mut self,
        painter: &egui::Painter,
        rect: egui::Rect,
        changes: Vec<Change>,
        frame: Option<FrameData>,
    );
    /// The newest picking answer the renderer gave since the last call.
    fn picked(&mut self) -> Option<Picking>;
    /// The bodies whose uploads the renderer refused since the last call: the pane draws
    /// them no more until their mesh changes.
    fn refused(&mut self) -> BTreeSet<u32>;
    /// Whether the renderer refused the last frame backing off
    /// ([`ViewRenderer::retry_later`]): the pane then asks for the next frame at once.
    fn backing_off(&mut self) -> bool {
        false
    }
}

/// What the pane and the paint callbacks share: the changes not yet made, one for each body
/// (the last asked: an upload replaces an earlier upload or forget of its body, and a forget
/// an earlier upload, so frames that are never painted, a window minimized, hold no more
/// than one mesh a body); the newest picking answer; the bodies refused; the last failure
/// said, so a renderer refusing every frame is said once, not each frame; and whether the
/// last frame was refused backing off.
#[derive(Default)]
struct Shared {
    changes: BTreeMap<u32, Change>,
    picked: Option<Picking>,
    refused: BTreeSet<u32>,
    failure: Option<String>,
    backing_off: bool,
}

impl Shared {
    fn queue(&mut self, changes: Vec<Change>) {
        for change in changes {
            let body = match &change {
                Change::Upload(body, _) | Change::Forget(body) => *body,
            };
            self.changes.insert(body, change);
        }
    }
}

fn locked(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    // Held only to move values in and out, on the UI thread or egui's paint: a panic
    // elsewhere leaves nothing half-written that matters.
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Frames drawn through egui_wgpu's paint callback, by the `R` installed in its
/// `callback_resources` ([`install`]). Changes wait until a callback is prepared, so a
/// frame egui doesn't paint loses none.
pub struct OnGpu<R> {
    shared: Arc<Mutex<Shared>>,
    renderer: PhantomData<fn() -> R>,
}

impl<R: ViewRenderer> OnGpu<R> {
    pub fn new() -> Self {
        OnGpu {
            shared: Arc::default(),
            renderer: PhantomData,
        }
    }
}

impl<R: ViewRenderer> Default for OnGpu<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// `renderer` put where the paint callbacks of an [`OnGpu<R>`] find it.
pub fn install<R: ViewRenderer>(render_state: &egui_wgpu::RenderState, renderer: R) {
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(renderer);
}

impl<R: ViewRenderer> Present for OnGpu<R> {
    fn present(
        &mut self,
        painter: &egui::Painter,
        rect: egui::Rect,
        changes: Vec<Change>,
        frame: Option<FrameData>,
    ) {
        locked(&self.shared).queue(changes);
        if let Some(frame) = frame {
            painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                Callback::<R> {
                    shared: Arc::clone(&self.shared),
                    frame,
                    renderer: PhantomData,
                },
            ));
        }
    }

    fn picked(&mut self) -> Option<Picking> {
        locked(&self.shared).picked.take()
    }

    fn refused(&mut self) -> BTreeSet<u32> {
        std::mem::take(&mut locked(&self.shared).refused)
    }

    fn backing_off(&mut self) -> bool {
        locked(&self.shared).backing_off
    }
}

/// One frame's paint callback.
struct Callback<R> {
    shared: Arc<Mutex<Shared>>,
    frame: FrameData,
    renderer: PhantomData<fn() -> R>,
}

/// A frame's preparation, its GPU's half aside: the changes waiting made; the frame drawn
/// without the bodies whose uploads were refused, and without highlights of a body it
/// doesn't draw (the renderer refuses a whole frame for one); a refusal said once until a
/// frame is drawn again; and a picking answer kept for the pane. With no renderer installed,
/// the changes are taken, every upload among them refused, so none waits for ever.
fn prepared<R: ViewRenderer>(
    renderer: Option<&mut R>,
    shared: &Mutex<Shared>,
    data: &FrameData,
) -> Vec<wgpu::CommandBuffer> {
    let changes: Vec<Change> = std::mem::take(&mut locked(shared).changes)
        .into_values()
        .collect();
    let Some(renderer) = renderer else {
        let uploads = changes.iter().filter_map(|change| match change {
            Change::Upload(body, _) => Some(*body),
            Change::Forget(_) => None,
        });
        locked(shared).refused.extend(uploads);
        return Vec::new();
    };
    let refused = apply(renderer, changes);
    let bodies: Vec<u32> = data
        .bodies
        .iter()
        .copied()
        .filter(|body| !refused.contains(body))
        .collect();
    let drawn = |list: &[(u32, Element)]| -> Vec<(u32, Element)> {
        list.iter()
            .copied()
            .filter(|(body, _)| bodies.binary_search(body).is_ok())
            .collect()
    };
    let highlights = drawn(&data.highlights);
    let hovered = drawn(&data.hovered);
    locked(shared).refused.extend(refused);
    let frame = Frame {
        bodies: &bodies,
        highlights: &highlights,
        hovered: &hovered,
        ..data.frame()
    };
    let prepared = renderer.prepare(&frame);
    // Set by a frame refused backing off; cleared by one drawn, or refused otherwise.
    locked(shared).backing_off = matches!(&prepared, Err(failure) if R::retry_later(failure));
    let buffers = match prepared {
        Ok(buffer) => {
            locked(shared).failure = None;
            vec![buffer]
        }
        Err(failure) if R::retry_later(&failure) => {
            tracing::debug!("the viewport's renderer is backing off: {failure}");
            Vec::new()
        }
        Err(failure) => {
            let said = failure.to_string();
            let mut shared = locked(shared);
            if shared.failure.as_deref() != Some(said.as_str()) {
                tracing::warn!("the viewport's renderer refused a frame: {said}");
                shared.failure = Some(said);
            }
            Vec::new()
        }
    };
    if let Some(picking) = renderer.take_picking() {
        locked(shared).picked = Some(picking);
    }
    buffers
}

/// The pane's whole rectangle in physical pixels, `[x, y, width, height]`, unclamped to the
/// window, as Rerun's bridge sets it: egui sets the viewport clamped to the window before
/// a callback paints, which would squeeze the view of a pane partly off it; unclamped, the
/// view keeps its scale and egui's scissor cuts it.
fn unclamped_viewport(info: &egui::PaintCallbackInfo) -> [f32; 4] {
    let ppp = info.pixels_per_point;
    let at = info.viewport;
    [
        at.min.x * ppp,
        at.min.y * ppp,
        at.width() * ppp,
        at.height() * ppp,
    ]
}

impl<R: ViewRenderer> egui_wgpu::CallbackTrait for Callback<R> {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        prepared(resources.get_mut::<R>(), &self.shared, &self.frame)
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(renderer) = resources.get::<R>() {
            let [x, y, width, height] = unclamped_viewport(&info);
            pass.set_viewport(x, y, width, height, 0.0, 1.0);
            renderer.composite(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame as the renderer was given it: its bodies, highlights, and hover.
    type Drawn = (Vec<u32>, Vec<(u32, Element)>, Vec<(u32, Element)>);

    #[derive(Default)]
    struct Counting {
        calls: Vec<String>,
        frames: Vec<Drawn>,
        picking: Option<Picking>,
        /// Refusing each frame as backing off, rather than as having no GPU.
        backing_off: bool,
    }

    impl ViewRenderer for Counting {
        type Failure = String;
        fn upload(&mut self, body: u32, mesh: &Arc<Mesh>) -> Result<(), String> {
            self.calls
                .push(format!("upload {body} {}", mesh.positions.len()));
            if mesh.positions.len() > 10 {
                return Err("out of memory".to_owned());
            }
            Ok(())
        }
        fn forget(&mut self, body: u32) {
            self.calls.push(format!("forget {body}"));
        }
        fn prepare(&mut self, frame: &Frame<'_>) -> Result<wgpu::CommandBuffer, String> {
            self.frames.push((
                frame.bodies.to_vec(),
                frame.highlights.to_vec(),
                frame.hovered.to_vec(),
            ));
            if self.backing_off {
                return Err(format!("backing off, {} frame(s)", self.frames.len()));
            }
            Err("no GPU here".to_owned())
        }
        fn retry_later(failure: &String) -> bool {
            failure.starts_with("backing off")
        }
        fn composite(&self, _: &mut wgpu::RenderPass<'_>) {}
        fn take_picking(&mut self) -> Option<Picking> {
            self.picking.take()
        }
    }

    fn mesh(points: usize) -> Arc<Mesh> {
        Arc::new(Mesh {
            positions: vec![[0.0; 3]; points],
            normals: vec![[0.0; 3]; points],
            triangles: Vec::new(),
            face_triangles: Vec::new(),
            valid: None,
            edge_points: Vec::new(),
            edge_point_counts: Vec::new(),
        })
    }

    #[test]
    fn changes_are_made_in_the_order_given_so_a_forget_then_an_upload_of_one_body_leaves_it_held() {
        // Two frames' changes queued before a paint: body 1 forgotten (gone at one
        // revision), then uploaded again (back at the next). Made out of order, the body
        // would end forgotten.
        let mut renderer = Counting::default();
        let refused = apply(
            &mut renderer,
            vec![
                Change::Upload(2, mesh(3)),
                Change::Forget(1),
                Change::Upload(1, mesh(4)),
            ],
        );
        assert_eq!(renderer.calls, ["upload 2 3", "forget 1", "upload 1 4"]);
        assert!(refused.is_empty());
    }

    #[test]
    fn a_refused_upload_is_returned_until_the_body_is_uploaded_or_forgotten_after_it() {
        let mut renderer = Counting::default();
        let refused = apply(
            &mut renderer,
            vec![Change::Upload(1, mesh(11)), Change::Upload(2, mesh(11))],
        );
        assert_eq!(refused.into_iter().collect::<Vec<_>>(), [1, 2]);
        let refused = apply(
            &mut renderer,
            vec![
                Change::Upload(1, mesh(11)),
                Change::Upload(1, mesh(3)),
                Change::Upload(2, mesh(11)),
                Change::Forget(2),
            ],
        );
        assert!(refused.is_empty());
    }

    #[test]
    fn a_picking_rectangle_is_taken_from_one_to_256_pixels_a_side_within_its_offset() {
        let rect = |min: [i32; 2], size: [u32; 2]| PixelRect { min, size };
        // silico-render's own rule, the one its frame's check refuses by.
        assert!(rect([0, 0], [1, 1]).takes());
        assert!(rect([-5, 3], [256, 256]).takes());
        assert!(rect([1 << 24, -(1 << 24)], [3, 3]).takes());
        assert!(!rect([0, 0], [0, 1]).takes());
        assert!(!rect([0, 0], [257, 1]).takes());
        assert!(!rect([(1 << 24) + 1, 0], [3, 3]).takes());
        assert!(!rect([0, -(1 << 24) - 1], [3, 3]).takes());
        assert!(!rect([i32::MIN, 0], [3, 3]).takes());
    }

    fn data(bodies: &[u32], highlights: &[(u32, Element)]) -> FrameData {
        FrameData {
            serial: 0,
            camera: Camera {
                projection: silico_protocol::Projection::Orthographic { view_height: 80.0 },
                eye: [0.0, -100.0, 0.0],
                target: [0.0; 3],
                up: [0.0, 0.0, 1.0],
            },
            size_px: [400, 300],
            pixels_per_point: 1.0,
            bodies: bodies.to_vec(),
            highlights: highlights.to_vec(),
            hovered: Vec::new(),
            outlines: Outlines::DEFAULT,
            picking: None,
        }
    }

    fn told(changes: &BTreeMap<u32, Change>) -> Vec<String> {
        changes
            .values()
            .map(|change| match change {
                Change::Upload(n, mesh) => format!("upload {n} {}", mesh.positions.len()),
                Change::Forget(n) => format!("forget {n}"),
            })
            .collect()
    }

    #[test]
    fn frames_never_painted_hold_one_change_a_body_the_last_asked() {
        // Two frames with no view (nothing painted): body 1 uploaded then forgotten, body 2
        // uploaded twice, body 3 forgotten then uploaded. Waiting, each body's last change,
        // in the bodies' order.
        let ctx = egui::Context::default();
        let painter = egui::Painter::new(ctx, egui::LayerId::background(), egui::Rect::EVERYTHING);
        let mut gpu = OnGpu::<Counting>::new();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0));
        gpu.present(
            &painter,
            rect,
            vec![
                Change::Upload(1, mesh(1)),
                Change::Upload(2, mesh(2)),
                Change::Forget(3),
            ],
            None,
        );
        gpu.present(
            &painter,
            rect,
            vec![
                Change::Forget(1),
                Change::Upload(2, mesh(5)),
                Change::Upload(3, mesh(6)),
            ],
            None,
        );
        assert_eq!(
            told(&locked(&gpu.shared).changes),
            ["forget 1", "upload 2 5", "upload 3 6"]
        );
        // Ten thousand more frames never painted, a mesh for each of two bodies each time:
        // still one change a body.
        for i in 0..10_000 {
            gpu.present(
                &painter,
                rect,
                vec![Change::Upload(1, mesh(i % 7)), Change::Upload(2, mesh(1))],
                None,
            );
        }
        assert_eq!(locked(&gpu.shared).changes.len(), 3);
    }

    #[test]
    fn a_frame_leaves_out_bodies_refused_now_and_their_highlights_and_keeps_its_picking() {
        // Body 2's mesh is refused as it's uploaded: the frame draws body 1 alone, with
        // only body 1's highlight and hover; the renderer's picking answer is kept for the
        // pane.
        let shared = Mutex::new(Shared::default());
        locked(&shared).queue(vec![
            Change::Upload(1, mesh(3)),
            Change::Upload(2, mesh(11)),
        ]);
        let pick = Picking {
            frame: 7,
            rect: PixelRect {
                min: [1, 2],
                size: [3, 3],
            },
            hit: None,
            edge: None,
        };
        let mut renderer = Counting {
            picking: Some(pick),
            ..Counting::default()
        };
        let face = (1, Element::Face(1));
        let edge = (2, Element::Edge(4));
        let frame = FrameData {
            hovered: vec![edge, face],
            ..data(&[1, 2], &[face, edge])
        };
        let made = prepared(Some(&mut renderer), &shared, &frame);
        assert!(made.is_empty());
        assert_eq!(renderer.frames, [(vec![1], vec![face], vec![face])]);
        assert_eq!(
            locked(&shared).refused.iter().copied().collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(locked(&shared).picked, Some(pick));
        assert!(locked(&shared).changes.is_empty());
    }

    #[test]
    fn the_bodies_a_callback_found_refused_are_given_to_the_pane_once() {
        // Body 2's mesh, of 11 points, is refused as the frame's callback is prepared; body
        // 1's is taken. The pane is told of body 2 alone, and asked again, of none.
        let ctx = egui::Context::default();
        let painter = egui::Painter::new(ctx, egui::LayerId::background(), egui::Rect::EVERYTHING);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0));
        let mut gpu = OnGpu::<Counting>::new();
        gpu.present(
            &painter,
            rect,
            vec![Change::Upload(1, mesh(3)), Change::Upload(2, mesh(11))],
            None,
        );
        assert!(gpu.refused().is_empty());
        let mut renderer = Counting::default();
        prepared(Some(&mut renderer), &gpu.shared, &data(&[1, 2], &[]));
        assert_eq!(gpu.refused().into_iter().collect::<Vec<_>>(), [2]);
        assert!(gpu.refused().is_empty());
    }

    #[test]
    fn a_renderer_refusing_every_frame_is_said_once_and_still_told_of_each_change() {
        let shared = Mutex::new(Shared::default());
        let mut renderer = Counting::default();
        for i in 0..100 {
            locked(&shared).queue(vec![Change::Upload(1, mesh(i % 5))]);
            assert!(prepared(Some(&mut renderer), &shared, &data(&[1], &[])).is_empty());
        }
        assert_eq!(renderer.frames.len(), 100);
        assert_eq!(renderer.calls.len(), 100);
        assert_eq!(locked(&shared).failure.as_deref(), Some("no GPU here"));
    }

    #[test]
    fn a_renderer_backing_off_is_neither_said_nor_taken_for_the_failure_said() {
        // A failure said, then frames refused backing off, each worded otherwise: the
        // failure said stays the GPU's, and the frames are still asked for and draw nothing.
        let shared = Mutex::new(Shared::default());
        let mut renderer = Counting::default();
        assert!(prepared(Some(&mut renderer), &shared, &data(&[1], &[])).is_empty());
        renderer.backing_off = true;
        for _ in 0..10 {
            assert!(prepared(Some(&mut renderer), &shared, &data(&[1], &[])).is_empty());
        }
        assert_eq!(renderer.frames.len(), 11);
        assert_eq!(locked(&shared).failure.as_deref(), Some("no GPU here"));
        // The pane is told it backs off, until a frame is refused otherwise.
        let mut gpu = OnGpu::<Counting>::new();
        gpu.shared = Arc::new(shared);
        assert!(gpu.backing_off());
        assert!(
            gpu.backing_off(),
            "until a frame is drawn or refused otherwise"
        );
        renderer.backing_off = false;
        assert!(prepared(Some(&mut renderer), &gpu.shared, &data(&[1], &[])).is_empty());
        assert!(!gpu.backing_off());
        // The real renderer's backing off is the one retried; its other failures aren't.
        assert!(<Renderer as ViewRenderer>::retry_later(
            &RenderFailure::BackingOff { frames: 2 }
        ));
        assert!(!<Renderer as ViewRenderer>::retry_later(
            &RenderFailure::OutOfMemory("4 GiB".to_owned())
        ));
    }

    /// A renderer that fails every frame, saying nothing of backing off: `retry_later` its
    /// trait's own.
    struct Failing;

    impl ViewRenderer for Failing {
        type Failure = String;
        fn upload(&mut self, _: u32, _: &Arc<Mesh>) -> Result<(), String> {
            Ok(())
        }
        fn forget(&mut self, _: u32) {}
        fn prepare(&mut self, _: &Frame<'_>) -> Result<wgpu::CommandBuffer, String> {
            Err("the device was lost".to_owned())
        }
        fn composite(&self, _: &mut wgpu::RenderPass<'_>) {}
        fn take_picking(&mut self) -> Option<Picking> {
            None
        }
    }

    #[test]
    fn a_renderers_failure_is_said_unless_it_says_to_retry_later() {
        // A renderer that doesn't say a failure is for the moment: each is a failure, kept
        // as said, and the pane isn't told the renderer backs off.
        assert!(!Failing::retry_later(&"backing off".to_owned()));
        let mut gpu = OnGpu::<Failing>::new();
        assert!(prepared(Some(&mut Failing), &gpu.shared, &data(&[], &[])).is_empty());
        assert_eq!(
            locked(&gpu.shared).failure.as_deref(),
            Some("the device was lost")
        );
        assert!(!gpu.backing_off());
    }

    #[test]
    fn with_no_renderer_installed_the_changes_are_taken_and_the_uploads_refused() {
        let shared = Mutex::new(Shared::default());
        locked(&shared).queue(vec![Change::Upload(1, mesh(3)), Change::Forget(2)]);
        assert!(prepared::<Counting>(None, &shared, &data(&[1], &[])).is_empty());
        assert!(locked(&shared).changes.is_empty());
        assert_eq!(
            locked(&shared).refused.iter().copied().collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn a_pane_partly_off_the_window_keeps_its_whole_viewport_unclamped() {
        // A 200 by 100 point pane at (−50, 700) in an 800 by 750 point window at 2×: egui's
        // clamped viewport would be 300 by 100 pixels from (0, 1400); the whole pane's is
        // 400 by 200 from (−100, 1400).
        let info = egui::PaintCallbackInfo {
            viewport: egui::Rect::from_min_size(egui::pos2(-50.0, 700.0), egui::vec2(200.0, 100.0)),
            clip_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 750.0)),
            pixels_per_point: 2.0,
            screen_size_px: [1600, 1500],
        };
        assert_eq!(unclamped_viewport(&info), [-100.0, 1400.0, 400.0, 200.0]);
        let clamped = info.viewport_in_pixels();
        assert_eq!(
            (clamped.left_px, clamped.width_px, clamped.height_px),
            (0, 300, 100)
        );
    }
}
