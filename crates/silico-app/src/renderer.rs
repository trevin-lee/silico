//! The viewport's renderer, as the pane sees it (docs/design/m5.0-app.md, section 10; M5.6):
//! bodies uploaded once by the pane, each frame's view (the camera, the size, the bodies to
//! draw, the highlights, and a picking rectangle), drawn into egui's render pass, and the
//! picking result a frame late. `silico-render`'s renderer, on eframe's device, is the one
//! this is for, and these types mirror its frame API's; it places every face and edge
//! itself from the camera, in `f64` (section 10, *The bodies move instead*), so the pane
//! passes the camera and no transform.
//!
//! The renderer lives in egui_wgpu's `callback_resources` ([`install`]), and each frame
//! reaches it through a paint callback ([`OnGpu`]): the pane's uploads and forgets, queued
//! in order until a callback is prepared, then the frame. A test hands the pane a
//! [`Present`] of its own, and [`apply`]s the changes to a renderer that needs no GPU.
//!
//! TODO(Worker 3's `render-frame-api`, stacked on #332): implement [`ViewRenderer`] for
//! `silico_render::render::Renderer` (its `RenderFailure` as [`ViewRenderer::Failure`]),
//! built on eframe's device, installed in `main.rs` with [`OnGpu`] as the app's
//! [`Present`], with a test drawing through it on lavapipe (the `wgpu` feature).

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui;
use eframe::egui_wgpu;
use eframe::wgpu;
use silico_protocol::{Camera, Mesh};

/// The widest and tallest picking rectangle a renderer takes, in physical pixels.
pub const MAX_PICKING_SIDE: u32 = 256;

/// The farthest a picking rectangle's corner may be from the frame's top left, in physical
/// pixels, either way.
pub const MAX_PICKING_OFFSET: i32 = 1 << 24;

/// A body's element, numbered from 1 as M4.6's ID buffer numbers them (`Face1`, `Edge1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Element {
    Face(u32),
    Edge(u32),
}

/// A rectangle of the viewport's image, in physical pixels from its top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub min: [i32; 2],
    pub size: [u32; 2],
}

impl PixelRect {
    /// Whether a renderer takes it: each side from 1 to [`MAX_PICKING_SIDE`], its corner
    /// within [`MAX_PICKING_OFFSET`] of the frame's top left.
    pub fn takes(&self) -> bool {
        self.size
            .iter()
            .all(|side| (1..=MAX_PICKING_SIDE).contains(side))
            && self
                .min
                .iter()
                .all(|at| (-MAX_PICKING_OFFSET..=MAX_PICKING_OFFSET).contains(at))
    }
}

/// What picking found nearest under a frame's [`PixelRect`]: the body by its number, its
/// element, the pixel, and the point in the document's coordinates, if the renderer gives
/// one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    pub body: u32,
    pub hit: Element,
    pub pixel: [u32; 2],
    pub point: Option<[f64; 3]>,
}

/// A picking rectangle's answer: the renderer's number of the frame that asked it, the
/// rectangle, and what was hit, or none for the background alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picking {
    pub frame: u64,
    pub rect: PixelRect,
    pub hit: Option<Pick>,
}

/// One frame of the viewport, as the renderer draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame<'a> {
    /// The tab's camera, as the app holds it (decision 29), in `f64`.
    pub camera: &'a Camera,
    /// The viewport's size in physical pixels.
    pub size_px: [u32; 2],
    /// Physical pixels per egui point: the display's scale.
    pub pixels_per_point: f32,
    /// The bodies to draw, by number, ascending, each once, each uploaded.
    pub bodies: &'a [u32],
    /// The elements drawn highlighted, as the selection gives them (M5.7).
    pub highlights: &'a [(u32, Element)],
    /// The rectangle to pick under, if one was asked: its answer comes a frame later.
    pub picking: Option<PixelRect>,
}

/// A [`Frame`] whose values it holds: what the pane hands on, to be drawn once the paint
/// callback is prepared.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameData {
    pub camera: Camera,
    pub size_px: [u32; 2],
    pub pixels_per_point: f32,
    pub bodies: Vec<u32>,
    pub highlights: Vec<(u32, Element)>,
    pub picking: Option<PixelRect>,
}

impl FrameData {
    /// The frame it holds.
    pub fn frame(&self) -> Frame<'_> {
        Frame {
            camera: &self.camera,
            size_px: self.size_px,
            pixels_per_point: self.pixels_per_point,
            bodies: &self.bodies,
            highlights: &self.highlights,
            picking: self.picking,
        }
    }
}

/// The pane's view of a renderer (M5.6), mirroring `silico_render`'s frame API.
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
    /// Draws what [`prepare`](Self::prepare) made into egui's render pass, its viewport
    /// already the pane's.
    fn composite(&self, pass: &mut wgpu::RenderPass<'_>);
    /// The answer to the rectangle a frame asked to pick under, from the frame after it;
    /// none until then.
    fn take_picking(&mut self) -> Option<Picking>;
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
}

/// What the pane and the paint callbacks share: the changes not yet made, one for each body
/// (the last asked: an upload replaces an earlier upload or forget of its body, and a forget
/// an earlier upload, so frames that are never painted, a window minimized, hold no more
/// than one mesh a body); the newest picking answer; the bodies refused; and the last
/// failure said, so a renderer refusing every frame is said once, not each frame.
#[derive(Default)]
struct Shared {
    changes: BTreeMap<u32, Change>,
    picked: Option<Picking>,
    refused: BTreeSet<u32>,
    failure: Option<String>,
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
    let highlights: Vec<(u32, Element)> = data
        .highlights
        .iter()
        .copied()
        .filter(|(body, _)| bodies.binary_search(body).is_ok())
        .collect();
    locked(shared).refused.extend(refused);
    let frame = Frame {
        bodies: &bodies,
        highlights: &highlights,
        ..data.frame()
    };
    let buffers = match renderer.prepare(&frame) {
        Ok(buffer) => {
            locked(shared).failure = None;
            vec![buffer]
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
            // The pane's whole rectangle, unclamped to the window, as Rerun's bridge sets
            // it: a viewport partly off screen keeps its scale, the scissor cutting it.
            let ppp = info.pixels_per_point;
            let at = info.viewport;
            pass.set_viewport(
                at.min.x * ppp,
                at.min.y * ppp,
                at.width() * ppp,
                at.height() * ppp,
                0.0,
                1.0,
            );
            renderer.composite(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame as the renderer was given it: its bodies and highlights.
    type Drawn = (Vec<u32>, Vec<(u32, Element)>);

    #[derive(Default)]
    struct Counting {
        calls: Vec<String>,
        frames: Vec<Drawn>,
        picking: Option<Picking>,
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
            self.frames
                .push((frame.bodies.to_vec(), frame.highlights.to_vec()));
            Err("no GPU here".to_owned())
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
        assert!(rect([0, 0], [1, 1]).takes());
        assert!(rect([-5, 3], [256, 256]).takes());
        assert!(rect([1 << 24, -(1 << 24)], [3, 3]).takes());
        assert!(!rect([0, 0], [0, 1]).takes());
        assert!(!rect([0, 0], [257, 1]).takes());
        assert!(!rect([(1 << 24) + 1, 0], [3, 3]).takes());
        assert!(!rect([0, -(1 << 24) - 1], [3, 3]).takes());
    }

    fn data(bodies: &[u32], highlights: &[(u32, Element)]) -> FrameData {
        FrameData {
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
        // only body 1's highlight; the renderer's picking answer is kept for the pane.
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
        };
        let mut renderer = Counting {
            picking: Some(pick),
            ..Counting::default()
        };
        let face = (1, Element::Face(1));
        let edge = (2, Element::Edge(4));
        let made = prepared(Some(&mut renderer), &shared, &data(&[1, 2], &[face, edge]));
        assert!(made.is_empty());
        assert_eq!(renderer.frames, [(vec![1], vec![face])]);
        assert_eq!(
            locked(&shared).refused.iter().copied().collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(locked(&shared).picked, Some(pick));
        assert!(locked(&shared).changes.is_empty());
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
}
