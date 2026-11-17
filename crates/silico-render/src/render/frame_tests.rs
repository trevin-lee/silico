//! The app's viewport's frames on lavapipe: bodies uploaded once and drawn from the camera,
//! picking read a frame later, composited into a pass, and hostile frames refused.
//!
//! The scene, by hand: the bench's 40 x 20 x 10 box from the origin (`box_mesh`), from the
//! front (`front_of_a_box`): orthographic, 44 mm over 48 rows (11/12 mm a pixel) about
//! (21, 10, 7), 64 by 48 pixels. A point (x, z) on it lands at column 32 + (x − 21) × 12/11
//! and row 24 − (z − 7) × 12/11: its front face (Face3, y = 0) from column 9.09 to 52.73
//! and row 20.73 to 31.64, its top edge (Edge3, z = 10) along row 20.73, 2 points wide.

use std::sync::Arc;
use std::time::Duration;

use silico_protocol::{Camera, MIN_EYE_DISTANCE, Mesh, Projection};

use super::tests::on_lavapipe;
use re_renderer::view_builder::{OrthographicCameraMode, Projection as ViewProjection};

use super::{
    BACKGROUND, BODY, CheckedFrame, EDGE, Element, Frame, HIGHLIGHT, HIT_EDGE_POINTS,
    HIT_VERTEX_POINTS, MAX_PENDING_PICKS, MAX_PICKING_SIDE, OUTPUT_FORMAT, Outlines, PendingPick,
    Pick, Pickable, Picking, PixelRect, RenderFailure, Renderer, Tint, hits, project, ray,
};
use crate::bench::{box_mesh, front_of_a_box};

const WIDTH: u32 = 64;
/// The serial [`frame`] gives a frame, and [`pick`] the frames after it one more: the
/// caller's numbers, any it likes.
const ASKING: u64 = 41;
const HEIGHT: u32 = 48;

fn the_box() -> Arc<Mesh> {
    Arc::new(box_mesh([0.0; 3], [40.0, 20.0, 10.0]))
}

fn front() -> Camera {
    front_of_a_box(WIDTH, HEIGHT).camera
}

fn frame<'a>(
    camera: &'a Camera,
    bodies: &'a [u32],
    highlights: &'a [(u32, Element)],
    picking: Option<PixelRect>,
) -> Frame<'a> {
    Frame {
        serial: ASKING,
        camera,
        size_px: [WIDTH, HEIGHT],
        pixels_per_point: 1.0,
        bodies,
        highlights,
        hovered: &[],
        outlines: Outlines::DEFAULT,
        tinted: &[],
        tint: Tint::NONE,
        picking,
        edges: true,
        vertices: None,
        background: VIEWPORT_BG,
        lines: &[],
        planes: &[],
    }
}

/// An opaque black behind the bodies, as the app's viewport's was before its gradient: the
/// app's frames have none now ([`NO_BACKGROUND`]).
const VIEWPORT_BG: Option<[u8; 3]> = Some([0, 0, 0]);

/// No background: the app's frames' (its theme's `VIEWPORT_FRAME_BG`), composited over the
/// gradient it paints under them.
const NO_BACKGROUND: Option<[u8; 3]> = None;

fn rect(min: [i32; 2], size: [u32; 2]) -> PixelRect {
    PixelRect { min, size }
}

/// Waits for the GPU: the test's, never the renderer's.
fn wait(r: &Renderer) {
    r.ctx
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(60)),
        })
        .unwrap();
}

/// `frame` prepared and submitted, as egui would, and the GPU waited for.
fn draw(r: &mut Renderer, frame: &Frame<'_>) {
    let commands = r.prepare(frame).unwrap();
    r.ctx.queue.submit([commands]);
    wait(r);
}

/// What `frame`'s picking finds: anything older answered and taken first (a frame drawn, so
/// a frame prepared and never submitted answers too, with whatever its readback held), then
/// the frame drawn, and none answered until the next frame begins; the next frame (without
/// picking) answers it.
fn pick(r: &mut Renderer, frame: &Frame<'_>) -> Picking {
    draw(
        r,
        &Frame {
            serial: frame.serial.wrapping_add(1),
            picking: None,
            ..*frame
        },
    );
    r.take_picking();
    let asked = frame.picking.unwrap();
    draw(r, frame);
    assert_eq!(r.take_picking(), None, "answered in its own frame");
    draw(
        r,
        &Frame {
            serial: frame.serial.wrapping_add(1),
            picking: None,
            ..*frame
        },
    );
    let picking = r.take_picking().expect("not answered a frame later");
    assert_eq!((picking.frame, picking.rect), (frame.serial, asked));
    assert_eq!(r.take_picking(), None, "answered twice");
    picking
}

/// The hit of `frame`'s picking, without its point.
fn hit(r: &mut Renderer, frame: &Frame<'_>) -> Option<(u32, Element, [u32; 2])> {
    pick(r, frame).hit.map(|p| (p.body, p.hit, p.pixel))
}

/// Inside the front face, clear of its edges: columns 30 to 34, rows 24 to 28, nearest the
/// centre (32.5, 26.5) the pixel (32, 26).
const ON_THE_FACE: PixelRect = PixelRect {
    min: [30, 24],
    size: [5, 5],
};
/// On the front face's top edge: row 20 (its centre 20.5, 0.23 from the edge's line, inside
/// its 1-point half width; above the face, which starts at 20.73).
const ON_THE_TOP_EDGE: PixelRect = PixelRect {
    min: [32, 20],
    size: [1, 1],
};

/// The front face's point at the centre of pixel (32, 26), by hand: (21 + 0.5 × 11/12, 0,
/// 7 − 2.5 × 11/12).
const CENTRE_OF_32_26: [f64; 3] = [21.0 + 5.5 / 12.0, 0.0, 7.0 - 27.5 / 12.0];

fn assert_point(pick: &Pick, want: [f64; 3], within: [f64; 3]) {
    let got = pick.point.expect("no point");
    for k in 0..3 {
        assert!(
            (got[k] - want[k]).abs() <= within[k],
            "{got:?} against {want:?}"
        );
    }
}

#[test]
fn the_renderer_can_live_in_eguis_callback_resources() {
    // egui_wgpu's CallbackResources (a concurrent type map) holds Send + Sync values only.
    fn shared<T: Send + Sync + 'static>() {}
    shared::<Renderer>();
}

#[test]
fn a_picking_rectangle_finds_the_face_and_the_edge_under_it_a_frame_later() {
    on_lavapipe(
        "render::frame_tests::a_picking_rectangle_finds_the_face_and_the_edge_under_it_a_frame_later",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let face = pick(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE)));
            let found = face.hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Face(3), [32, 26])
            );
            // The point at the pixel's centre, (32.5, 26.5): x = 21 + 0.5 × 11/12 = 21.4583,
            // z = 7 − 2.5 × 11/12 = 4.7083, on the face (y = 0).
            assert_point(&found, CENTRE_OF_32_26, [1e-3, 1e-3, 1e-3]);
            // The top edge, in front of the back face's (Edge7, along the same line 20 mm
            // behind it): its point at the centre (32.5, 20.5), x = 21.4583 and z = 7 + 3.5 ×
            // 11/12 = 10.2083, at the edge's depth less its offset toward the eye.
            let edge = pick(r, &frame(&camera, &[1], &[], Some(ON_THE_TOP_EDGE)));
            let found = edge.hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Edge(3), [32, 20])
            );
            assert_point(
                &found,
                [21.0 + 5.5 / 12.0, 0.0, 7.0 + 38.5 / 12.0],
                [1e-3, 1e-4, 1e-3],
            );
            // The background, above the box (row 5, z = 24.4): no hit, answered as such.
            let above = pick(r, &frame(&camera, &[1], &[], Some(rect([30, 3], [5, 5]))));
            assert_eq!(above.hit, None);
        },
    );
}

#[test]
fn bodies_upload_once_and_any_number_of_frames_from_any_camera_upload_nothing() {
    on_lavapipe(
        "render::frame_tests::bodies_upload_once_and_any_number_of_frames_from_any_camera_upload_nothing",
        |r| {
            // Uploading gives the GPU nothing: the first frame that draws a face uploads it.
            r.upload(1, &the_box()).unwrap();
            assert_eq!(r.uploads(), 0);
            let front = front();
            draw(r, &frame(&front, &[1], &[], None));
            assert_eq!(r.uploads(), 6);
            // From above (Face6 at the centre), from the right in perspective (Face2), and
            // the front again: the same meshes, moved.
            let top = Camera {
                eye: [21.0, 10.0, 60.0],
                up: [0.0, 1.0, 0.0],
                ..front.clone()
            };
            let right = Camera {
                projection: Projection::Perspective {
                    field_of_view: 40.0,
                },
                eye: [120.0, 10.0, 5.0],
                target: [40.0, 10.0, 5.0],
                up: [0.0, 0.0, 1.0],
            };
            let centre = Some(rect([32, 24], [1, 1]));
            for (camera, face) in [(&top, 6), (&right, 2), (&front, 3), (&top, 6), (&front, 3)] {
                let found = hit(r, &frame(camera, &[1], &[], centre));
                assert_eq!(found.map(|h| (h.0, h.1)), Some((1, Element::Face(face))));
            }
            assert_eq!(r.uploads(), 6);
        },
    );
}

#[test]
fn a_body_forgotten_is_drawn_no_more_and_one_replaced_is_uploaded_again() {
    on_lavapipe(
        "render::frame_tests::a_body_forgotten_is_drawn_no_more_and_one_replaced_is_uploaded_again",
        |r| {
            // Body 2, the same box 30 mm nearer the eye (y −30 to −10): in front of body 1,
            // its front face (Face3) at y = −30.
            let camera = front();
            r.upload(1, &the_box()).unwrap();
            r.upload(
                2,
                &Arc::new(box_mesh([0.0, -30.0, 0.0], [40.0, 20.0, 10.0])),
            )
            .unwrap();
            let both = pick(r, &frame(&camera, &[1, 2], &[], Some(ON_THE_FACE)));
            let found = both.hit.unwrap();
            assert_eq!((found.body, found.hit), (2, Element::Face(3)));
            let [x, _, z] = CENTRE_OF_32_26;
            assert_point(&found, [x, -30.0, z], [1e-3, 1e-3, 1e-3]);
            // Left out of a frame, it isn't drawn; forgotten, it can't be.
            let found = hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE)));
            assert_eq!(found, Some((1, Element::Face(3), [32, 26])));
            r.forget(2);
            r.forget(7);
            let Err(RenderFailure::Invalid(refused)) =
                r.prepare(&frame(&camera, &[1, 2], &[], None))
            else {
                panic!("a forgotten body drawn");
            };
            assert_eq!(refused.parameter, "bodies");
            let found = hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE)));
            assert_eq!(found, Some((1, Element::Face(3), [32, 26])));
            // The same mesh again, by its Arc or an equal one, uploads nothing; another, the
            // box moved 100 mm right (out of view: from column 32 + 79 × 12/11 = 118), drops
            // the old meshes, and isn't placed where the view can't see it: nothing uploaded
            // until a view does.
            let before = r.uploads();
            let mesh = the_box();
            r.upload(1, &mesh).unwrap();
            r.upload(1, &mesh).unwrap();
            draw(r, &frame(&camera, &[1], &[], None));
            assert_eq!(r.uploads(), before);
            r.upload(
                1,
                &Arc::new(box_mesh([100.0, 0.0, 0.0], [40.0, 20.0, 10.0])),
            )
            .unwrap();
            assert_eq!(hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))), None);
            assert_eq!(r.uploads(), before);
        },
    );
}

#[test]
fn highlights_change_no_picking_id() {
    on_lavapipe(
        "render::frame_tests::highlights_change_no_picking_id",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // In any order, repeated: the front face and its top edge.
            let lit = [
                (1, Element::Edge(3)),
                (1, Element::Face(3)),
                (1, Element::Edge(3)),
            ];
            for picking in [ON_THE_FACE, ON_THE_TOP_EDGE] {
                let plain = hit(r, &frame(&camera, &[1], &[], Some(picking)));
                let highlighted = hit(r, &frame(&camera, &[1], &lit, Some(picking)));
                assert!(plain.is_some());
                assert_eq!(highlighted, plain);
            }
            // Row 19 (its centre 19.5, 1.23 from the edge's line): past the edge's 1-point
            // half width, inside the worker's highlighted 2. A frame's highlight is an
            // outline alone, so its edge is no wider: the background still.
            let beside = Some(rect([32, 19], [1, 1]));
            assert_eq!(hit(r, &frame(&camera, &[1], &[], beside)), None);
            assert_eq!(hit(r, &frame(&camera, &[1], &lit, beside)), None);
            // The whole ID buffer, drawn by the same path for the worker, with the face and
            // the edge highlighted: every pixel the same but those the highlighted edge, 4
            // points wide where it was 2, now covers (Edge3, over what was the face, the
            // background, or at the corners Edge2's and Edge4's ends beside it).
            let scene = front_of_a_box(WIDTH, HEIGHT);
            let plain = r.render(&scene).unwrap().ids.unwrap();
            let mut lit = scene.clone();
            lit.bodies[0].highlight_faces = vec![2];
            lit.bodies[0].highlight_edges = vec![2];
            let highlighted = r.render(&lit).unwrap().ids.unwrap();
            let edge3 = [1, 0x8000_0003];
            assert!(plain.contains(&[1, 3]) && plain.contains(&edge3));
            let mut wider = 0;
            for (i, (was, is)) in plain.iter().zip(&highlighted).enumerate() {
                if was != is {
                    assert!(*is == edge3, "pixel {i}: {was:?} became {is:?}");
                    wider += 1;
                }
            }
            assert!(wider > 0, "the highlighted edge is no wider");
            // Every Edge3 pixel of the plain buffer is Edge3 still.
            assert!(
                plain
                    .iter()
                    .zip(&highlighted)
                    .all(|(was, is)| *was != edge3 || *is == edge3)
            );
        },
    );
}

#[test]
fn a_frame_of_no_bodies_picks_nothing() {
    on_lavapipe(
        "render::frame_tests::a_frame_of_no_bodies_picks_nothing",
        |r| {
            let camera = front();
            assert_eq!(hit(r, &frame(&camera, &[], &[], Some(ON_THE_FACE))), None);
            // A body uploaded but not drawn isn't picked either.
            r.upload(1, &the_box()).unwrap();
            assert_eq!(hit(r, &frame(&camera, &[], &[], Some(ON_THE_FACE))), None);
            assert_eq!(r.uploads(), 0);
        },
    );
}

#[test]
fn a_picking_rectangle_off_the_frame_finds_only_what_the_frame_shows() {
    // The camera's target 20 mm right (41, 10, 7): the front face from column 32 − 41 × 12/11
    // = −12.7 to 32 − 12/11 = 30.9, past the frame's left side, where re_renderer draws on
    // as if the frame were larger. Then 22 mm left (−1, 10, 7): from column 33.1 to 76.7,
    // past its right side.
    on_lavapipe(
        "render::frame_tests::a_picking_rectangle_off_the_frame_finds_only_what_the_frame_shows",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let left = Camera {
                eye: [41.0, -50.0, 7.0],
                target: [41.0, 10.0, 7.0],
                ..front()
            };
            // Wholly outside, over the face: nothing.
            let outside = Some(rect([-8, 24], [4, 4]));
            assert_eq!(hit(r, &frame(&left, &[1], &[], outside)), None);
            // Partly: its centre (−4 + 2.5) at column −2, outside; the nearest of the frame's
            // own pixels is column 0, two away, on row 26.
            let partly = Some(rect([-4, 24], [5, 5]));
            assert_eq!(
                hit(r, &frame(&left, &[1], &[], partly)),
                Some((1, Element::Face(3), [0, 26]))
            );
            let right = Camera {
                eye: [-1.0, -50.0, 7.0],
                target: [-1.0, 10.0, 7.0],
                ..front()
            };
            // Its centre at column 64, outside: the nearest inside is column 63.
            let partly = Some(rect([62, 24], [5, 5]));
            assert_eq!(
                hit(r, &frame(&right, &[1], &[], partly)),
                Some((1, Element::Face(3), [63, 26]))
            );
            // Below the frame (rows 48 on), over where the face would be: nothing.
            let below = Some(rect([40, 48], [4, 4]));
            assert_eq!(hit(r, &frame(&right, &[1], &[], below)), None);
        },
    );
}

#[test]
fn frames_prepared_and_never_submitted_leave_the_next_one_whole() {
    on_lavapipe(
        "render::frame_tests::frames_prepared_and_never_submitted_leave_the_next_one_whole",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            for _ in 0..5 {
                // Dropped: neither submitted nor composited.
                r.prepare(&frame(&camera, &[1], &[], Some(ON_THE_FACE)))
                    .unwrap();
            }
            assert_eq!(r.uploads(), 6);
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))),
                Some((1, Element::Face(3), [32, 26]))
            );
            assert_eq!(r.uploads(), 6);
        },
    );
}

/// A target of the renderer's format, `frame` composited into it after the frame's command
/// buffer (if the frame was prepared): its pixels, RGBA, rows top to bottom.
fn composited(r: &mut Renderer, commands: Option<wgpu::CommandBuffer>) -> Vec<[u8; 4]> {
    composited_over(r, commands, wgpu::Color::TRANSPARENT)
}

/// [`composited`], into a pass cleared to `under` first, as egui's pass holds what it
/// painted under the frame.
fn composited_over(
    r: &mut Renderer,
    commands: Option<wgpu::CommandBuffer>,
    under: wgpu::Color,
) -> Vec<[u8; 4]> {
    composited_sized(r, commands, under, [WIDTH, HEIGHT])
}

/// [`composited_over`], into a pass `size` pixels: each row read back padded to the 256 bytes
/// a copy asks, and the padding dropped.
fn composited_sized(
    r: &mut Renderer,
    commands: Option<wgpu::CommandBuffer>,
    under: wgpu::Color,
    [width, height]: [u32; 2],
) -> Vec<[u8; 4]> {
    let device = r.ctx.device.clone();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("composite target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("composite readback"),
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui's pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(under),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        r.composite(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let mut buffers: Vec<wgpu::CommandBuffer> = commands.into_iter().collect();
    buffers.push(encoder.finish());
    r.ctx.queue.submit(buffers);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |mapped| mapped.unwrap());
    wait(r);
    let data = buffer.slice(..).get_mapped_range().unwrap();
    data.chunks_exact(usize::try_from(row).unwrap())
        .flat_map(|line| line[..usize::try_from(width * 4).unwrap()].chunks_exact(4))
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect()
}

#[test]
fn a_frame_is_composited_into_the_pass_and_a_refused_one_not_at_all() {
    on_lavapipe(
        "render::frame_tests::a_frame_is_composited_into_the_pass_and_a_refused_one_not_at_all",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let commands = r.prepare(&frame(&camera, &[1], &[], None)).unwrap();
            let pixels = composited(r, Some(commands));
            let at = |x: u32, y: u32| pixels[usize::try_from(y * WIDTH + x).unwrap()];
            // The corner is the frame's background, opaque; the centre is the front face, not
            // it.
            let corner = at(1, 1);
            assert_eq!(corner, [0, 0, 0, 255]);
            assert_ne!(at(32, 26), corner);
            assert_eq!(at(32, 26)[3], 255);
            // Composited again, without another frame: the same frame.
            assert_eq!(composited(r, None), pixels);
            // A refused frame leaves nothing to composite: the pass keeps its clear.
            let mut broken = camera.clone();
            broken.eye = broken.target;
            assert!(r.prepare(&frame(&broken, &[1], &[], None)).is_err());
            assert!(composited(r, None).iter().all(|p| *p == [0, 0, 0, 0]));
        },
    );
}

#[test]
fn a_frames_background_is_the_colour_it_gives_and_a_renders_its_own() {
    on_lavapipe(
        "render::frame_tests::a_frames_background_is_the_colour_it_gives_and_a_renders_its_own",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // Each colour at the corner, off the box, to 1 of 255 (sRGB to linear and back);
            // the box's front face at the centre the same whatever's behind it.
            let mut centres = vec![];
            for background in [[0, 0, 0], [255, 255, 255], [12, 200, 77], [230, 233, 237]] {
                let commands = r
                    .prepare(&Frame {
                        background: Some(background),
                        ..frame(&camera, &[1], &[], None)
                    })
                    .unwrap();
                let pixels = composited(r, Some(commands));
                let at = |x: u32, y: u32| pixels[usize::try_from(y * WIDTH + x).unwrap()];
                for (x, y) in [(1, 1), (62, 46), (32, 5)] {
                    let got = at(x, y);
                    for k in 0..3 {
                        assert!(
                            got[k].abs_diff(background[k]) <= 1,
                            "{background:?} at ({x}, {y}): {got:?}"
                        );
                    }
                    assert_eq!(got[3], 255, "{background:?} at ({x}, {y}): {got:?}");
                }
                centres.push(at(32, 26));
            }
            assert!(centres.windows(2).all(|w| w[0] == w[1]), "{centres:?}");
            // None: composited over what the pass holds, each colour cleared there showing
            // off the box exactly (the frame's alpha 0 there, so the pass's own bytes), the
            // box's front face over it as opaque as with a background of its own.
            for under in [[0x1e, 0x1f, 0x22], [0x3a, 0x3c, 0x41], [12, 200, 77]] {
                let commands = r
                    .prepare(&Frame {
                        background: NO_BACKGROUND,
                        ..frame(&camera, &[1], &[], None)
                    })
                    .unwrap();
                let [red, green, blue] = under.map(|c| f64::from(c) / 255.0);
                let clear = wgpu::Color {
                    r: red,
                    g: green,
                    b: blue,
                    a: 1.0,
                };
                let pixels = composited_over(r, Some(commands), clear);
                let at = |x: u32, y: u32| pixels[usize::try_from(y * WIDTH + x).unwrap()];
                let [red, green, blue] = under;
                for (x, y) in [(1, 1), (62, 46), (32, 5)] {
                    assert_eq!(at(x, y), [red, green, blue, 255], "{under:?} at ({x}, {y})");
                }
                assert_eq!(at(32, 26), centres[0], "{under:?}: the face");
            }
            // A render keeps its own, the light grey.
            let [red, green, blue] = BACKGROUND;
            let rendered = r.render(&front_of_a_box(WIDTH, HEIGHT)).unwrap();
            let mut reader = png::Decoder::new(std::io::Cursor::new(rendered.png))
                .read_info()
                .unwrap();
            let mut rgb = vec![0; reader.output_buffer_size().unwrap()];
            reader.next_frame(&mut rgb).unwrap();
            assert_eq!(rgb[..3], [red, green, blue]);
        },
    );
}

#[test]
fn a_frame_with_no_background_blends_an_edges_rim_over_the_pass_premultiplied_once() {
    // The box's top edge (Edge3, z = 10) along row 20.73, 2 points wide: rows 19.73 to
    // 21.73, so row 20 is under it whole and row 19 only its feathered outer rim, over
    // nothing (the front face starts at row 20.73), columns 14 to 48 clear of the box's
    // sides. The edge's colour is EDGE, #282828. With no background, the frame holds that
    // rim premultiplied: its alpha α its coverage, its colour EDGE · α. Composited over a
    // pass cleared to an opaque `under`, each channel is then EDGE · α + under · (1 − α), α
    // read over nothing (the frame's alpha there), to 1.5 of 255 for the 8-bit rounding of
    // the frame, the composite, and α. Blended as alpha to coverage, the rim's colour is
    // multiplied by α a second time: at α = 30/255, EDGE's 40 is 4.7 premultiplied once,
    // but 1.0 twice (40 to linear, 0.0212, times α, 0.0025, back to sRGB, 8.2, times α), a
    // dark fringe.
    on_lavapipe(
        "render::frame_tests::a_frame_with_no_background_blends_an_edges_rim_over_the_pass_premultiplied_once",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let none = || Frame {
                background: NO_BACKGROUND,
                ..frame(&camera, &[1], &[], None)
            };
            let commands = r.prepare(&none()).unwrap();
            let over_nothing = composited(r, Some(commands));
            let pixel = |pixels: &[[u8; 4]], x: u32, y: u32| {
                pixels[usize::try_from(y * WIDTH + x).unwrap()]
            };
            for under in [[0x1e, 0x1f, 0x22], [200, 200, 200], [12, 200, 77]] {
                let commands = r.prepare(&none()).unwrap();
                let [red, green, blue] = under.map(|c| f64::from(c) / 255.0);
                let clear = wgpu::Color {
                    r: red,
                    g: green,
                    b: blue,
                    a: 1.0,
                };
                let over = composited_over(r, Some(commands), clear);
                for x in 14..=48 {
                    // The edge whole: its own colour, opaque.
                    let line = pixel(&over, x, 20);
                    for k in 0..3 {
                        assert!(
                            line[k].abs_diff(EDGE[k]) <= 1,
                            "{under:?}: ({x}, 20) {line:?}"
                        );
                    }
                    let rim = pixel(&over_nothing, x, 19);
                    let alpha = f64::from(rim[3]) / 255.0;
                    assert!(
                        rim[3] > 10 && rim[3] < 245,
                        "({x}, 19) over nothing {rim:?}: not the rim"
                    );
                    let got = pixel(&over, x, 19);
                    assert_eq!(got[3], 255, "{under:?}: ({x}, 19) {got:?}");
                    for k in 0..3 {
                        let want = f64::from(EDGE[k]) * alpha + f64::from(under[k]) * (1.0 - alpha);
                        assert!(
                            (f64::from(got[k]) - want).abs() <= 1.5,
                            "{under:?}: ({x}, 19) {got:?}, channel {k}, against {want} (α {alpha})"
                        );
                        // Over nothing, the rim premultiplied once.
                        let once = f64::from(EDGE[k]) * alpha;
                        assert!(
                            (f64::from(rim[k]) - once).abs() <= 1.5,
                            "({x}, 19) over nothing {rim:?}, channel {k}, against {once}"
                        );
                    }
                }
            }
        },
    );
}

/// The parameter `frame` is refused for.
fn refused(r: &mut Renderer, frame: &Frame<'_>) -> &'static str {
    match r.prepare(frame) {
        Err(RenderFailure::Invalid(invalid)) => invalid.parameter,
        Err(other) => panic!("refused as {other}"),
        Ok(_) => panic!("{frame:?} drawn"),
    }
}

#[test]
fn hostile_frames_are_refused_naming_what_and_leave_the_renderer_drawing() {
    on_lavapipe(
        "render::frame_tests::hostile_frames_are_refused_naming_what_and_leave_the_renderer_drawing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let good = frame(&camera, &[1], &[], None);
            let largest = r.ctx.device.limits().max_texture_dimension_2d;
            for size_px in [[0, 0], [0, 48], [64, 0], [largest + 1, 1], [1, u32::MAX]] {
                assert_eq!(
                    refused(r, &Frame { size_px, ..good }),
                    "size_px",
                    "{size_px:?}"
                );
            }
            for ppp in [
                32.0,
                0.0,
                -1.0,
                f32::NAN,
                f32::INFINITY,
                32.5,
                f32::MIN_POSITIVE / 2.0,
            ] {
                let hostile = Frame {
                    pixels_per_point: ppp,
                    ..good
                };
                if ppp > 0.0 && ppp.is_finite() && ppp <= 32.0 {
                    // A subnormal is above 0: drawn, its lines a sliver.
                    r.prepare(&hostile).unwrap();
                } else {
                    assert_eq!(refused(r, &hostile), "pixels_per_point", "{ppp}");
                }
            }
            for bodies in [&[2][..], &[1, 1], &[0], &[u32::MAX]] {
                assert_eq!(refused(r, &frame(&camera, bodies, &[], None)), "bodies");
            }
            r.upload(2, &the_box()).unwrap();
            assert_eq!(refused(r, &frame(&camera, &[2, 1], &[], None)), "bodies");
            for highlight in [
                (1, Element::Face(0)),
                (1, Element::Face(7)),
                (1, Element::Edge(0)),
                (1, Element::Edge(13)),
                (1, Element::Face(u32::MAX)),
                (2, Element::Face(1)),
                (9, Element::Face(1)),
            ] {
                let lit = [highlight];
                assert_eq!(
                    refused(r, &frame(&camera, &[1], &lit, None)),
                    "highlights",
                    "{highlight:?}"
                );
            }
            for picking in [
                rect([0, 0], [0, 1]),
                rect([0, 0], [1, 0]),
                rect([0, 0], [MAX_PICKING_SIDE + 1, 1]),
                rect([i32::MIN, 0], [1, 1]),
                rect([0, i32::MAX], [1, 1]),
                rect([(1 << 24) + 1, 0], [1, 1]),
            ] {
                assert_eq!(
                    refused(r, &frame(&camera, &[1], &[], Some(picking))),
                    "picking",
                    "{picking:?}"
                );
            }
            let mut on_target = camera.clone();
            on_target.eye = on_target.target;
            assert_eq!(
                refused(r, &frame(&on_target, &[1], &[], None)),
                "camera.eye"
            );
            let mut nan = camera.clone();
            nan.up = [f64::NAN, 0.0, 1.0];
            assert_eq!(refused(r, &frame(&nan, &[1], &[], None)), "camera.up");
            // Too close a look for the box's reach from the target, √(21² + 10² + 7²) =
            // 24.3 mm: below 2.43e-4 mm high. With no bodies there's no reach to keep to.
            let mut close = camera.clone();
            close.projection = Projection::Orthographic {
                view_height: 2.4e-4,
            };
            assert_eq!(
                refused(r, &frame(&close, &[1], &[], None)),
                "camera.view_height"
            );
            r.prepare(&frame(&close, &[], &[], None)).unwrap();
            // The largest frame and picking rectangle there are are drawn.
            r.prepare(&Frame {
                size_px: [largest, 1],
                picking: Some(rect([-(1 << 24), 0], [MAX_PICKING_SIDE, MAX_PICKING_SIDE])),
                ..good
            })
            .unwrap();
            // After every refusal the renderer still draws, and picks.
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))),
                Some((1, Element::Face(3), [32, 26]))
            );
        },
    );
}

#[test]
fn a_hostile_upload_is_refused_and_the_body_keeps_its_mesh() {
    on_lavapipe(
        "render::frame_tests::a_hostile_upload_is_refused_and_the_body_keeps_its_mesh",
        |r| {
            let camera = front();
            let Err(RenderFailure::Invalid(zero)) = r.upload(0, &the_box()) else {
                panic!("body 0 uploaded");
            };
            assert_eq!(zero.parameter, "body");
            r.upload(1, &the_box()).unwrap();
            let mut hostile = Vec::new();
            let mut nan = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            nan.positions[5][1] = f64::NAN;
            hostile.push(nan);
            let mut far = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            far.edge_points[0][0] = 2e12;
            hostile.push(far);
            let mut past = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            past.triangles[3][2] = 24;
            hostile.push(past);
            let mut short = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            short.normals.pop();
            hostile.push(short);
            let mut miscounted = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            miscounted.face_triangles[0] = 3;
            hostile.push(miscounted);
            // A vertex's point as an edge's: not finite, or past the coordinates' bound.
            for bad in [f64::NAN, f64::INFINITY, -2e12] {
                let mut vertex = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
                vertex.vertex_points[7][2] = bad;
                hostile.push(vertex);
            }
            for mesh in hostile {
                let Err(RenderFailure::Invalid(invalid)) = r.upload(1, &Arc::new(mesh)) else {
                    panic!("a hostile mesh uploaded");
                };
                assert_eq!(invalid.parameter, "mesh");
                assert!(
                    invalid.message.starts_with("body 1's mesh: "),
                    "{}",
                    invalid.message
                );
            }
            // Body 1 is still the box.
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))),
                Some((1, Element::Face(3), [32, 26]))
            );
            // An empty mesh is a body with nothing to draw, or pick.
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
            r.upload(1, &Arc::new(empty)).unwrap();
            assert_eq!(hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))), None);
        },
    );
}

#[test]
fn of_many_frames_picking_and_none_taken_the_newest_answer_is_given_once() {
    on_lavapipe(
        "render::frame_tests::of_many_frames_picking_and_none_taken_the_newest_answer_is_given_once",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // Twenty frames, each picking one pixel further right along row 26 (columns 12
            // to 31, all on the front face), their requests kept from the frames after them,
            // so no answer is read until all are back: then given to one frame together (the
            // readback belt keeps an unread answer for two frames, so the last three are
            // there to choose from).
            let mut asked = std::collections::BTreeMap::new();
            for column in 12..32 {
                let picking = Some(rect([column, 26], [1, 1]));
                draw(r, &frame(&camera, &[1], &[], picking));
                asked.append(&mut r.pending);
            }
            assert_eq!(asked.len(), 20);
            r.pending = asked;
            draw(r, &frame(&camera, &[1], &[], None));
            let newest = r.take_picking().unwrap();
            assert_eq!(newest.rect, rect([31, 26], [1, 1]));
            let found = newest.hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Face(3), [31, 26])
            );
            // Nothing older is given after it, however many frames follow.
            assert!(r.pending.is_empty());
            for _ in 0..3 {
                draw(r, &frame(&camera, &[1], &[], None));
                assert_eq!(r.take_picking(), None);
            }
        },
    );
}

#[test]
fn requests_never_answered_are_dropped_oldest_first_past_the_bound() {
    on_lavapipe(
        "render::frame_tests::requests_never_answered_are_dropped_oldest_first_past_the_bound",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // Twenty requests no readback will answer (identifiers past any the renderer
            // gives in this test), as a driver that never answers leaves them.
            let first = r.readback + 1000;
            for id in first..first + 20 {
                r.pending.insert(
                    id,
                    PendingPick {
                        serial: 0,
                        bodies: vec![],
                        rect: ON_THE_FACE,
                        size: [WIDTH, HEIGHT],
                        frame: camera.frame().unwrap(),
                        origin: camera.target,
                        depth: 1.0,
                        projection: ViewProjection::Orthographic {
                            camera_mode: OrthographicCameraMode::NearPlaneCenter,
                            vertical_world_size: 44.0,
                            far_plane_distance: 100.0,
                        },
                        perspective: false,
                        span: 44.0,
                        strip_fraction: super::edge_fraction(HEIGHT),
                    },
                );
            }
            // A frame's harvest keeps the newest MAX_PENDING_PICKS of them, then its own
            // request; the next answers it, and the eight are left.
            draw(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE)));
            let kept: Vec<u64> = (first + 20 - 8..first + 20).collect();
            assert_eq!(MAX_PENDING_PICKS, 8);
            assert_eq!(r.pending.len(), 9);
            assert_eq!(r.pending.keys().skip(1).copied().collect::<Vec<_>>(), kept);
            draw(r, &frame(&camera, &[1], &[], None));
            let found = r.take_picking().unwrap().hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Face(3), [32, 26])
            );
            assert_eq!(r.pending.keys().copied().collect::<Vec<_>>(), kept);
        },
    );
}

#[test]
fn a_late_answer_never_names_a_body_forgotten_or_replaced_since_its_frame() {
    on_lavapipe(
        "render::frame_tests::a_late_answer_never_names_a_body_forgotten_or_replaced_since_its_frame",
        |r| {
            let camera = front();
            let one = frame(&camera, &[1], &[], Some(ON_THE_FACE));
            let after = |r: &mut Renderer, bodies: &[u32]| -> Picking {
                let next = Frame {
                    serial: 1,
                    ..frame(&camera, bodies, &[], None)
                };
                draw(r, &next);
                r.take_picking().expect("no answer")
            };
            r.upload(1, &the_box()).unwrap();
            // Asked of the box, then its mesh replaced (moved 1 mm: still under the
            // rectangle, so only the generation tells): the answer comes, naming nothing,
            // with the asking frame's serial.
            draw(r, &one);
            let asked = one.serial;
            r.upload(1, &Arc::new(box_mesh([1.0, 0.0, 0.0], [40.0, 20.0, 10.0])))
                .unwrap();
            let answer = after(r, &[1]);
            assert_eq!(
                (answer.frame, answer.rect, answer.hit),
                (asked, ON_THE_FACE, None)
            );
            // Forgotten: the same.
            draw(r, &one);
            r.forget(1);
            assert_eq!(after(r, &[]).hit, None);
            // Forgotten and uploaded again under its number: another mesh, all the same.
            r.upload(1, &the_box()).unwrap();
            draw(r, &one);
            r.forget(1);
            r.upload(1, &the_box()).unwrap();
            assert_eq!(after(r, &[1]).hit, None);
            // An equal mesh uploaded again changes nothing: still named.
            draw(r, &one);
            r.upload(1, &the_box()).unwrap();
            let found = after(r, &[1]).hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Face(3), [32, 26])
            );
            // Of two bodies, a replaced one is passed over for the nearest still as drawn:
            // body 2, x −8 to −3 (columns 0.4 to 5.8), left of the box (from 9.09), the
            // rectangle's centre at column 8.5 over the box's left edge.
            r.upload(2, &Arc::new(box_mesh([-8.0, 0.0, 0.0], [5.0, 20.0, 10.0])))
                .unwrap();
            let between = Some(rect([4, 24], [9, 5]));
            let both = frame(&camera, &[1, 2], &[], between);
            assert_eq!(pick(r, &both).hit.map(|h| h.body), Some(1));
            draw(r, &both);
            r.upload(1, &Arc::new(box_mesh([1.0, 0.0, 0.0], [40.0, 20.0, 10.0])))
                .unwrap();
            let found = after(r, &[1, 2]).hit.unwrap();
            assert_eq!(found.body, 2, "{found:?}");
            assert!(found.pixel[0] <= 6, "{found:?}");
        },
    );
}

#[test]
fn a_perspective_picks_point_is_the_pixels_centre_on_the_face() {
    on_lavapipe(
        "render::frame_tests::a_perspective_picks_point_is_the_pixels_centre_on_the_face",
        |r| {
            // The front view in perspective, 40° high, from (21, −50, 7): the front face 50
            // mm along the view. Pixel (32, 26)'s centre is at x = 1/64 and y = −5/48 of the
            // half view, so by hand at x = 21 + 50 × (1/64) × tan 20° × 4/3, z = 7 − 50 ×
            // (5/48) × tan 20°, y = 0: (21.3791, 0, 5.1043).
            r.upload(1, &the_box()).unwrap();
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 40.0,
                },
                ..front()
            };
            let found = pick(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE)))
                .hit
                .unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Face(3), [32, 26])
            );
            let tan = libm::tan(20f64.to_radians());
            let want = [
                21.0 + 50.0 / 64.0 * tan * 4.0 / 3.0,
                0.0,
                7.0 - 50.0 * 5.0 / 48.0 * tan,
            ];
            assert!((want[0] - 21.3791).abs() < 1e-4 && (want[2] - 5.1043).abs() < 1e-4);
            assert_point(&found, want, [1e-3, 1e-3, 1e-3]);
        },
    );
}

/// `a`'s length.
fn norm(a: [f64; 3]) -> f64 {
    a.iter().map(|c| c * c).sum::<f64>().sqrt()
}

/// Where the ray `(origin, direction)` meets the plane where coordinate `k` is `at`.
fn meets((origin, direction): ([f64; 3], [f64; 3]), k: usize, at: f64) -> [f64; 3] {
    let t = (at - origin[k]) / direction[k];
    std::array::from_fn(|i| origin[i] + direction[i] * t)
}

/// `p`'s distance from the line of the ray `(origin, direction)`, its direction of unit
/// length.
fn off_the_ray(p: [f64; 3], (origin, direction): ([f64; 3], [f64; 3])) -> f64 {
    let d: [f64; 3] = std::array::from_fn(|k| p[k] - origin[k]);
    let t: f64 = (0..3).map(|k| d[k] * direction[k]).sum();
    norm(std::array::from_fn(|k| d[k] - direction[k] * t))
}

fn assert_near(got: [f64; 3], want: [f64; 3], relative: f64) {
    let off = norm(std::array::from_fn(|k| got[k] - want[k]));
    assert!(
        off <= relative * norm(want),
        "{got:?} against {want:?}: {off:e} off"
    );
}

/// The front camera in perspective, 40° high.
fn front_in_perspective() -> Camera {
    Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        ..front()
    }
}

/// From above, in front, and to the right of the box, looking at (20, 10, 5).
fn from_the_corner(projection: Projection) -> Camera {
    Camera {
        projection,
        eye: [70.0, -45.0, 55.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

#[test]
fn the_ray_through_the_images_centre_is_the_view_axis() {
    // The image's centre is half its size, (32, 24): the ray from the eye toward the target
    // in perspective; orthographic, from the target along the view. The way, by hand, from
    // the eye to the target.
    let size = [WIDTH, HEIGHT];
    let centre = [32.0, 24.0];
    let ortho = Projection::Orthographic { view_height: 60.0 };
    let wide = Projection::Perspective {
        field_of_view: 60.0,
    };
    for camera in [
        front(),
        front_in_perspective(),
        from_the_corner(ortho),
        from_the_corner(wide),
    ] {
        let (origin, direction) = ray(&camera, size, centre).unwrap();
        let way: [f64; 3] = std::array::from_fn(|k| camera.target[k] - camera.eye[k]);
        let way = way.map(|c| c / norm(way));
        assert_near(direction, way, 1e-15);
        assert!((norm(direction) - 1.0).abs() <= 1e-15);
        let want = match camera.projection {
            Projection::Perspective { .. } => camera.eye,
            Projection::Orthographic { .. } => camera.target,
        };
        assert_near(origin, want, 1e-15);
    }
    // The front view's, exactly: along +y.
    let (_, direction) = ray(&front(), size, centre).unwrap();
    assert_eq!(direction.map(|c| c + 0.0), [0.0, 1.0, 0.0]);
}

#[test]
fn a_ray_through_a_pixel_meets_the_face_where_its_worked_by_hand() {
    // The ray is the GPU's projection, whose matrix is f32 (each entry to 6e-8 of it), so it
    // meets the hand's worked points to 6e-8 of their offsets across the view (a few mm
    // here): to 1e-8 of the points, which are 20 mm and more from the origin.
    let size = [WIDTH, HEIGHT];
    // Orthographic, from the front: through pixel (32, 26)'s centre, (32.5, 26.5), parallel
    // to the view (+y), from the target's plane (y = 10), so meeting the front face (y = 0)
    // at (21 + 0.5 × 11/12, 0, 7 − 2.5 × 11/12).
    let (origin, direction) = ray(&front(), size, [32.5, 26.5]).unwrap();
    assert_eq!(direction.map(|c| c + 0.0), [0.0, 1.0, 0.0]);
    assert_near(origin, [21.0 + 5.5 / 12.0, 10.0, 7.0 - 27.5 / 12.0], 1e-8);
    assert_near(meets((origin, direction), 1, 0.0), CENTRE_OF_32_26, 1e-8);
    // In perspective, 40° high from (21, −50, 7): from the eye, meeting the face 50 mm
    // along the view at x = 21 + 50 × (1/64) × tan 20° × 4/3, z = 7 − 50 × (5/48) × tan 20°.
    let camera = front_in_perspective();
    let (origin, direction) = ray(&camera, size, [32.5, 26.5]).unwrap();
    assert_eq!(origin, camera.eye);
    let tan = libm::tan(20f64.to_radians());
    let want = [
        21.0 + 50.0 / 64.0 * tan * 4.0 / 3.0,
        0.0,
        7.0 - 50.0 * 5.0 / 48.0 * tan,
    ];
    assert_near(meets((origin, direction), 1, 0.0), want, 1e-8);
    // The image's corner (0, 0), by hand: up tan 20° and left tan 20° × 4/3 for each mm;
    // its offsets across the view are about half its length, so to 5e-8.
    let (_, corner) = ray(&camera, size, [0.0, 0.0]).unwrap();
    let way = [-tan * 4.0 / 3.0, 1.0, tan];
    assert_near(corner, way.map(|c| c / norm(way)), 5e-8);
}

#[test]
fn a_ray_is_none_for_a_camera_or_size_the_checks_refuse() {
    let pixel = [1.0, 1.0];
    assert_eq!(ray(&front(), [0, HEIGHT], pixel), None);
    assert_eq!(ray(&front(), [WIDTH, 0], pixel), None);
    for bad in [f64::NAN, f64::INFINITY] {
        assert_eq!(ray(&front(), [WIDTH, HEIGHT], [bad, 1.0]), None);
        assert_eq!(ray(&front(), [WIDTH, HEIGHT], [1.0, bad]), None);
    }
    let at_the_target = Camera {
        eye: front().target,
        ..front()
    };
    let no_up = Camera {
        up: [0.0, 1.0, 0.0],
        ..front()
    };
    let flat = Camera {
        projection: Projection::Orthographic { view_height: 0.0 },
        ..front()
    };
    let no_angle = Camera {
        projection: Projection::Perspective { field_of_view: 0.0 },
        ..front()
    };
    let nan_eye = Camera {
        eye: [f64::NAN, -50.0, 7.0],
        ..front()
    };
    for camera in [at_the_target, no_up, flat, no_angle, nan_eye] {
        assert!(camera.check(WIDTH, HEIGHT).is_err());
        assert_eq!(ray(&camera, [WIDTH, HEIGHT], pixel), None, "{camera:?}");
    }
    // A pixel outside the image is a ray all the same.
    assert!(ray(&front(), [WIDTH, HEIGHT], [-10.0, 1e6]).is_some());
}

#[test]
fn a_ray_through_a_pixel_far_off_the_image_is_finite_or_none() {
    // Pixels out to f64's largest, on each side, of a small image of a tall orthographic
    // view (so a pixel's offset across it is the most), from the front and from the corner
    // (where the view's right and up add on one axis), and in perspective: each ray given
    // is finite, its direction of unit length; and those past f64 are none.
    let tall = Projection::Orthographic { view_height: 1e9 };
    let wide = Projection::Perspective {
        field_of_view: 60.0,
    };
    let huge = [1e300, 1e306, 1e307, 4e307, 1e308, f64::MAX];
    let mut nones = 0;
    for camera in [
        Camera {
            projection: tall,
            ..front()
        },
        from_the_corner(tall),
        from_the_corner(wide),
    ] {
        for &x in &huge {
            for &y in &huge {
                for [sx, sy] in [[1.0, 1.0], [1.0, -1.0], [-1.0, 1.0], [-1.0, -1.0]] {
                    match ray(&camera, [2, 2], [sx * x, sy * y]) {
                        Some((origin, direction)) => {
                            assert!(
                                origin.iter().chain(&direction).all(|c| c.is_finite()),
                                "{camera:?} at ({x}, {y}): {origin:?} {direction:?}"
                            );
                            assert!((norm(direction) - 1.0).abs() <= 1e-15);
                        }
                        None => nones += 1,
                    }
                }
            }
        }
    }
    assert!(nones > 0);
    // An orthographic origin past f64 though its offsets across the view aren't: from the
    // corner, 0.99 of f64's largest right and as much down (half the view, 5e8 mm, for each
    // unit of the image's device coordinates, which are the pixel less 1 across and 1 less
    // it up), whose sum on the document's x (the view's right 0.740 of it, its up −0.375)
    // is 1.10 of it: none, where it was an origin of infinity.
    let corner = from_the_corner(tall);
    let d = 0.99 * f64::MAX / 5e8;
    assert_eq!(ray(&corner, [2, 2], [1.0 + d, 1.0 + d]), None);
    // And inside, 0.8 of it each way (0.89 on x): finite.
    let d = 0.8 * f64::MAX / 5e8;
    let (origin, _) = ray(&corner, [2, 2], [1.0 + d, 1.0 + d]).unwrap();
    assert!(origin.iter().all(|c| c.is_finite()), "{origin:?}");
}

/// `got` within `within` pixels of `want`, on each axis.
fn assert_pixel(got: Option<[f64; 2]>, want: [f64; 2], within: f64) {
    let got = got.expect("not projected");
    assert!(
        (0..2).all(|k| (got[k] - want[k]).abs() <= within),
        "{got:?} against {want:?}"
    );
}

#[test]
fn the_projection_is_the_cameras_with_its_near_plane_half_the_least_eye_distance() {
    // By the design (docs/design/m4.6-render.md, *A given camera*): what's drawn is cut to
    // depth MIN_EYE_DISTANCE and more, and the near plane is half that, so no triangle drawn
    // crosses it. The angle in radians and the image's aspect, f32, as the GPU takes them.
    let ViewProjection::Perspective {
        vertical_fov,
        near_plane_distance,
        aspect_ratio,
    } = super::view_projection(
        Projection::Perspective {
            field_of_view: 40.0,
        },
        [WIDTH, HEIGHT],
        0.0,
    )
    else {
        panic!("not a perspective");
    };
    assert_eq!(near_plane_distance, 5e-7f32);
    assert_eq!(aspect_ratio, 4.0f32 / 3.0);
    assert_eq!(vertical_fov, 40f64.to_radians() as f32);
    let ViewProjection::Orthographic {
        camera_mode,
        vertical_world_size,
        far_plane_distance,
    } = super::view_projection(
        Projection::Orthographic { view_height: 44.0 },
        [WIDTH, HEIGHT],
        3.5,
    )
    else {
        panic!("not orthographic");
    };
    assert!(matches!(
        camera_mode,
        OrthographicCameraMode::NearPlaneCenter
    ));
    assert_eq!((vertical_world_size, far_plane_distance), (44.0, 3.5));
}

#[test]
fn a_box_corner_projects_to_the_pixel_worked_by_hand() {
    // The corner (0, 0, 10), its top front left. From the front, orthographic: column
    // 32 + (0 − 21) × 12/11, row 24 − (10 − 7) × 12/11. To 2e-6 of a pixel: the GPU's
    // matrix is f32, its entries to 6e-8 of them, and the offsets are some 20 pixels.
    let size = [WIDTH, HEIGHT];
    let corner = [0.0, 0.0, 10.0];
    let want: [f64; 2] = [32.0 - 21.0 * 12.0 / 11.0, 24.0 - 3.0 * 12.0 / 11.0];
    assert!((want[0] - 9.0909).abs() < 1e-4 && (want[1] - 20.7273).abs() < 1e-4);
    assert_pixel(project(&front(), size, corner), want, 2e-6);
    // In perspective, 40° high from (21, −50, 7): 50 mm along the view, 21 left and 3 up,
    // so tan 20° of half the height (24 rows) for each unit, and tan 20° × 4/3 of half the
    // width (32 columns).
    let tan = libm::tan(20f64.to_radians());
    let want = [
        32.0 - 21.0 / 50.0 / (tan * 4.0 / 3.0) * 32.0,
        24.0 - 3.0 / 50.0 / tan * 24.0,
    ];
    assert_pixel(project(&front_in_perspective(), size, corner), want, 2e-6);
    // A point off the image lands off it: (−100, 0, 7), 121 mm left, from the front.
    assert_pixel(
        project(&front(), size, [-100.0, 0.0, 7.0]),
        [32.0 - 121.0 * 12.0 / 11.0, 24.0],
        1e-5,
    );
}

#[test]
fn a_point_on_a_pixels_ray_projects_back_to_the_pixel() {
    // Pixels across the image and off it, at depths along each ray from a micrometre to
    // ten metres (and before the view's plane, orthographic): back to the pixel, to 1e-9
    // of it.
    let size = [WIDTH, HEIGHT];
    let ortho = Projection::Orthographic { view_height: 60.0 };
    let wide = Projection::Perspective {
        field_of_view: 60.0,
    };
    let pixels = [
        [0.0, 0.0],
        [0.5, 0.5],
        [32.0, 24.0],
        [63.5, 47.5],
        [64.0, 48.0],
        [13.25, 40.75],
        [-20.0, 70.0],
    ];
    for camera in [
        front(),
        front_in_perspective(),
        from_the_corner(ortho),
        from_the_corner(wide),
    ] {
        let perspective = matches!(camera.projection, Projection::Perspective { .. });
        for pixel in pixels {
            let (origin, direction) = ray(&camera, size, pixel).unwrap();
            let mut along = vec![1e-3, 1.0, 37.5, 1e4];
            if !perspective {
                along.extend([0.0, -50.0]);
            }
            for t in along {
                let p: [f64; 3] = std::array::from_fn(|k| origin[k] + direction[k] * t);
                let got = project(&camera, size, p).unwrap();
                let length = |[x, y]: [f64; 2]| (x * x + y * y).sqrt();
                let scale = length(pixel).max(1.0);
                let off = length([got[0] - pixel[0], got[1] - pixel[1]]);
                assert!(
                    off <= 1e-9 * scale,
                    "{camera:?} {pixel:?} at {t}: {got:?}, {off:e} off"
                );
            }
        }
    }
}

#[test]
fn project_is_none_behind_the_eye_where_the_drawing_cuts_and_for_what_the_checks_refuse() {
    let size = [WIDTH, HEIGHT];
    // From the origin along +y: a point MIN_EYE_DISTANCE along the view projects (the
    // drawing's pyramid holds depth ≥ MIN_EYE_DISTANCE), one an ulp nearer doesn't, nor the
    // eye, nor what's behind it.
    let at_the_origin = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [0.0; 3],
        target: [0.0, 10.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let nearer = f64::from_bits(MIN_EYE_DISTANCE.to_bits() - 1);
    assert_pixel(
        project(&at_the_origin, size, [0.0, MIN_EYE_DISTANCE, 0.0]),
        [32.0, 24.0],
        1e-9,
    );
    for y in [nearer, 0.0, -1.0, -1e9] {
        assert_eq!(project(&at_the_origin, size, [0.0, y, 0.0]), None, "{y}");
    }
    // Orthographic has no behind: from the front, a point 1 km behind the eye projects.
    assert_pixel(
        project(&front(), size, [21.0, -1e6, 7.0]),
        [32.0, 24.0],
        1e-9,
    );
    // Not finite, in or out.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for k in 0..3 {
            let mut p = [20.0, 0.0, 5.0];
            p[k] = bad;
            assert_eq!(project(&front(), size, p), None);
            assert_eq!(project(&front_in_perspective(), size, p), None);
        }
    }
    let narrow = Camera {
        projection: Projection::Orthographic { view_height: 1e-3 },
        ..front()
    };
    assert!(project(&narrow, size, [1e300, 0.0, 7.0]).is_some());
    assert_eq!(project(&narrow, size, [f64::MAX, 0.0, 7.0]), None);
    // A camera or size the checks refuse.
    let at_the_target = Camera {
        eye: front().target,
        ..front()
    };
    assert_eq!(project(&at_the_target, size, [20.0, 0.0, 5.0]), None);
    assert_eq!(project(&front(), [0, HEIGHT], [20.0, 0.0, 5.0]), None);
    assert_eq!(project(&front(), [WIDTH, 0], [20.0, 0.0, 5.0]), None);
    // In perspective a height of 0 passes the camera's check (its corner's sec² and its
    // pyramid's slope both infinite), and the projection's x scale is then 0 and its y
    // pixels 0 high: a pixel (32, 0) of nothing, if the size weren't refused first.
    let at = [21.0, 0.0, 7.0];
    assert!(front_in_perspective().check(WIDTH, 0).is_ok());
    assert_eq!(project(&front_in_perspective(), [WIDTH, 0], at), None);
    assert_eq!(project(&front_in_perspective(), [0, HEIGHT], at), None);
}

#[test]
fn a_faces_projected_centre_is_that_face_in_a_frames_picking() {
    on_lavapipe(
        "render::frame_tests::a_faces_projected_centre_is_that_face_in_a_frames_picking",
        |r| {
            r.upload(1, &the_box()).unwrap();
            // Close enough that each face seen is many pixels across, its centre clear of its
            // edges' 2-pixel strokes.
            let ortho = Projection::Orthographic { view_height: 40.0 };
            let wide = Projection::Perspective {
                field_of_view: 30.0,
            };
            // The box's faces' centres: x high (Face2), y low (Face3), z high (Face6). From
            // the front only Face3 is seen; from the corner all three.
            let centres = [
                (2, [40.0, 10.0, 5.0]),
                (3, [20.0, 0.0, 5.0]),
                (6, [20.0, 10.0, 10.0]),
            ];
            for (camera, seen) in [
                (front(), &[3][..]),
                (front_in_perspective(), &[3][..]),
                (from_the_corner(ortho), &[2, 3, 6][..]),
                (from_the_corner(wide), &[2, 3, 6][..]),
            ] {
                for &(face, centre) in centres.iter().filter(|(f, _)| seen.contains(f)) {
                    let at = project(&camera, [WIDTH, HEIGHT], centre).unwrap();
                    let pixel = at.map(|c| c.floor() as i32);
                    let found = hit(r, &frame(&camera, &[1], &[], Some(rect(pixel, [1, 1]))));
                    assert_eq!(
                        found.map(|(body, element, _)| (body, element)),
                        Some((1, Element::Face(face))),
                        "{camera:?}: Face{face}'s centre at {at:?}"
                    );
                }
            }
        },
    );
}

#[test]
fn a_ray_passes_through_the_point_a_frames_picking_finds_at_its_pixel() {
    on_lavapipe(
        "render::frame_tests::a_ray_passes_through_the_point_a_frames_picking_finds_at_its_pixel",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let centre = Some(rect([32, 24], [1, 1]));
            let ortho = Projection::Orthographic { view_height: 60.0 };
            let wide = Projection::Perspective {
                field_of_view: 60.0,
            };
            // The front face's pixel (32, 26) from the front, both ways, and the image's
            // centre from the corner, both ways: the ID buffer's face, and its point on the
            // pixel's ray, off it by at most 1e-9 of its distance from the origin (along the
            // ray, the point is the GPU's f32 depth).
            for (camera, picking, face) in [
                (front(), Some(ON_THE_FACE), 3),
                (front_in_perspective(), Some(ON_THE_FACE), 3),
                (from_the_corner(ortho), centre, 6),
                (from_the_corner(wide), centre, 6),
            ] {
                let found = pick(r, &frame(&camera, &[1], &[], picking)).hit.unwrap();
                assert_eq!(
                    (found.body, found.hit),
                    (1, Element::Face(face)),
                    "{camera:?}"
                );
                let pixel = found.pixel.map(|c| f64::from(c) + 0.5);
                let line = ray(&camera, [WIDTH, HEIGHT], pixel).unwrap();
                let point = found.point.unwrap();
                let off = off_the_ray(point, line);
                assert!(
                    off <= 1e-9 * norm(point),
                    "{camera:?}: {point:?} {off:e} off"
                );
                // And where the ray meets the face's plane is that point, to the depth's f32.
                let on_the_face = match face {
                    3 => meets(line, 1, 0.0),
                    _ => meets(line, 2, 10.0),
                };
                assert_near(on_the_face, point, 1e-5);
            }
        },
    );
}

#[test]
fn a_hover_moving_across_every_face_and_edge_uploads_nothing() {
    on_lavapipe(
        "render::frame_tests::a_hover_moving_across_every_face_and_edge_uploads_nothing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            draw(r, &frame(&camera, &[1], &[], None));
            assert_eq!(r.uploads(), 6);
            // Each of the six faces and twelve edges highlighted in turn, then several at
            // once, then none: an outline, never a baked mesh.
            let mut hovers: Vec<Vec<(u32, Element)>> = (1..=6)
                .map(|n| vec![(1, Element::Face(n))])
                .chain((1..=12).map(|n| vec![(1, Element::Edge(n))]))
                .collect();
            hovers.push(vec![
                (1, Element::Face(3)),
                (1, Element::Face(6)),
                (1, Element::Edge(3)),
            ]);
            hovers.push(vec![]);
            for hover in &hovers {
                draw(r, &frame(&camera, &[1], hover, Some(ON_THE_FACE)));
            }
            assert_eq!(r.uploads(), 6);
            assert!(
                r.bodies[&1]
                    .faces
                    .iter()
                    .all(|f| f.baked.is_none() && f.plain.is_some())
            );
        },
    );
}

/// The distance from the centre of pixel `i` (of a frame `WIDTH` wide) to the segment from
/// `a` to `b`, in pixels.
fn from_segment(i: usize, a: [f64; 2], b: [f64; 2]) -> f64 {
    let i = u32::try_from(i).unwrap();
    let p = [f64::from(i % WIDTH) + 0.5, f64::from(i / WIDTH) + 0.5];
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]))
        .clamp(0.0, 1.0);
    let q = [a[0] + t * d[0] - p[0], a[1] + t * d[1] - p[1]];
    (q[0] * q[0] + q[1] * q[1]).sqrt()
}

/// The oblique view of the box: orthographic, 40 mm over 48 rows (5/6 mm a pixel), from
/// (1, −1, 1) about the box's centre (20, 10, 5), z up. Its axes by hand: right (1, 1, 0)/√2,
/// up (−1, 1, 2)/√6.
fn oblique() -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 40.0 },
        eye: [80.0, -50.0, 65.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

/// Where `p` lands in [`oblique`]'s frame, in pixels: column 32 + right · (p − target) × 6/5,
/// row 24 − up · (p − target) × 6/5.
fn oblique_px(p: [f64; 3]) -> [f64; 2] {
    let d = [p[0] - 20.0, p[1] - 10.0, p[2] - 5.0];
    let right = (d[0] + d[1]) / 2f64.sqrt();
    let up = (-d[0] + d[1] + 2.0 * d[2]) / 6f64.sqrt();
    [32.0 + right * 1.2, 24.0 - up * 1.2]
}

/// The box's faces [`oblique`] sees, by number, and their corners in order round them: Face2
/// (x = 40), Face3 (y = 0), Face6 (z = 10).
fn seen_faces() -> [(u32, [[f64; 3]; 4]); 3] {
    [
        (
            2,
            [
                [40.0, 0.0, 0.0],
                [40.0, 20.0, 0.0],
                [40.0, 20.0, 10.0],
                [40.0, 0.0, 10.0],
            ],
        ),
        (
            3,
            [
                [0.0, 0.0, 0.0],
                [40.0, 0.0, 0.0],
                [40.0, 0.0, 10.0],
                [0.0, 0.0, 10.0],
            ],
        ),
        (
            6,
            [
                [0.0, 0.0, 10.0],
                [40.0, 0.0, 10.0],
                [40.0, 20.0, 10.0],
                [0.0, 20.0, 10.0],
            ],
        ),
    ]
}

/// The distance from the centre of pixel `i` to the point `q`, in pixels.
fn from_point(i: usize, q: [f64; 2]) -> f64 {
    let i = u32::try_from(i).unwrap();
    let p = [f64::from(i % WIDTH) + 0.5, f64::from(i / WIDTH) + 0.5];
    let d = [p[0] - q[0], p[1] - q[1]];
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

#[test]
fn a_highlight_outlines_its_instance_alone_and_changes_nothing_else() {
    // Seen obliquely, each face's outline is its own parallelogram (Face3's, by hand: (6.5,
    // 24), (40.5, 43.6), (40.5, 33.8), (6.5, 14.2)), so a face's highlight is told from its
    // neighbours' and from its edges'. A face's highlight changes pixels only within 3.5 of
    // its border (the outline is 1.5 points wide, with its antialiasing: a loose check,
    // pixels aren't results), and some within 1 of each of its four sides' midpoints (its
    // top edge, Edge3, at most 3 points thick with its outline, passes 4.2 from Face3's
    // short sides' midpoints, and 8.5 from its far side's).
    on_lavapipe(
        "render::frame_tests::a_highlight_outlines_its_instance_alone_and_changes_nothing_else",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = oblique();
            let mut image = |lit: &[(u32, Element)]| {
                let commands = r.prepare(&frame(&camera, &[1], lit, None)).unwrap();
                composited(r, Some(commands))
            };
            let plain = image(&[]);
            let changed = |other: &[[u8; 4]]| -> Vec<usize> {
                (0..plain.len()).filter(|&i| other[i] != plain[i]).collect()
            };
            let sides = |corners: [[f64; 3]; 4]| -> Vec<([f64; 2], [f64; 2])> {
                (0..4)
                    .map(|k| (oblique_px(corners[k]), oblique_px(corners[(k + 1) % 4])))
                    .collect()
            };
            let midpoints_near = |sides: &[([f64; 2], [f64; 2])], changed: &[usize]| -> usize {
                sides
                    .iter()
                    .filter(|(a, b)| {
                        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                        changed.iter().any(|&i| from_point(i, mid) <= 1.0)
                    })
                    .count()
            };
            for (number, corners) in seen_faces() {
                let border = sides(corners);
                let lit = image(&[(1, Element::Face(number))]);
                let changed = changed(&lit);
                assert!(
                    changed.len() > 40,
                    "Face{number}: {} pixels changed",
                    changed.len()
                );
                for &i in &changed {
                    let off = border
                        .iter()
                        .map(|&(a, b)| from_segment(i, a, b))
                        .fold(f64::INFINITY, f64::min);
                    assert!(off <= 3.5, "Face{number}: pixel {i}, {off} from its border");
                }
                assert_eq!(midpoints_near(&border, &changed), 4, "Face{number}");
                // Its inside, as uploaded: the colour at its centre is the plain frame's.
                let centre = oblique_px(std::array::from_fn(|k| {
                    corners.iter().map(|c| c[k]).sum::<f64>() / 4.0
                }));
                let at = usize::try_from(centre[1] as u32 * WIDTH + centre[0] as u32).unwrap();
                assert_eq!(lit[at], plain[at], "Face{number}'s inside");
            }
            // The top edge alone (Face3's top side): about its line, and not round Face3.
            let (_, face3) = seen_faces()[1];
            let (a, b) = (oblique_px(face3[2]), oblique_px(face3[3]));
            let edge = changed(&image(&[(1, Element::Edge(3))]));
            assert!(edge.len() > 20, "{} pixels changed", edge.len());
            for &i in &edge {
                let off = from_segment(i, a, b);
                assert!(off <= 3.5, "pixel {i}, {off} from Edge3");
            }
            assert!(midpoints_near(&sides(face3), &edge) < 4);
            // And none: the plain frame again.
            assert_eq!(image(&[]), plain);
        },
    );
}

#[test]
fn an_answer_already_read_back_is_judged_against_the_bodies_when_its_taken() {
    on_lavapipe(
        "render::frame_tests::an_answer_already_read_back_is_judged_against_the_bodies_when_its_taken",
        |r| {
            let camera = front();
            let one = frame(&camera, &[1], &[], Some(ON_THE_FACE));
            // Frame N asks; frame N + 1 reads the answer back while body 1 is as drawn;
            // then the body changes, and only then is the answer taken.
            let read_back = |r: &mut Renderer, asking: &Frame<'_>| -> u64 {
                draw(r, asking);
                draw(
                    r,
                    &Frame {
                        serial: 1,
                        picking: None,
                        ..*asking
                    },
                );
                assert!(r.picked.is_some(), "not read back yet");
                asking.serial
            };
            r.upload(1, &the_box()).unwrap();
            // Replaced (moved 1 mm, still under the rectangle): no hit, the answer given.
            let asked = read_back(r, &one);
            r.upload(1, &Arc::new(box_mesh([1.0, 0.0, 0.0], [40.0, 20.0, 10.0])))
                .unwrap();
            let answer = r.take_picking().expect("no answer");
            assert_eq!(
                (answer.frame, answer.rect, answer.hit),
                (asked, ON_THE_FACE, None)
            );
            // Forgotten: the same.
            read_back(r, &one);
            r.forget(1);
            assert_eq!(r.take_picking().expect("no answer").hit, None);
            // Unchanged: named.
            r.upload(1, &the_box()).unwrap();
            read_back(r, &one);
            let found = r.take_picking().unwrap().hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Face(3), [32, 26])
            );
            // Of two bodies (body 2 left of the box, as in the test before), the other body's
            // pixel in the rectangle is the answer once the nearer one is replaced.
            r.upload(2, &Arc::new(box_mesh([-8.0, 0.0, 0.0], [5.0, 20.0, 10.0])))
                .unwrap();
            let both = frame(&camera, &[1, 2], &[], Some(rect([4, 24], [9, 5])));
            read_back(r, &both);
            r.upload(1, &Arc::new(box_mesh([1.0, 0.0, 0.0], [40.0, 20.0, 10.0])))
                .unwrap();
            let found = r.take_picking().unwrap().hit.unwrap();
            assert_eq!(found.body, 2, "{found:?}");
            assert!(found.pixel[0] <= 6, "{found:?}");
        },
    );
}

#[test]
fn an_answer_carries_the_serial_its_caller_gave_the_frame_that_asked() {
    on_lavapipe(
        "render::frame_tests::an_answer_carries_the_serial_its_caller_gave_the_frame_that_asked",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let asking = |serial: u64, picking: PixelRect| Frame {
                serial,
                ..frame(&camera, &[1], &[], Some(picking))
            };
            // Out of order, and repeated: the caller's numbers, echoed as given.
            for (serial, picking) in [
                (7, ON_THE_FACE),
                (3, ON_THE_TOP_EDGE),
                (3, ON_THE_FACE),
                (u64::MAX, ON_THE_FACE),
                (0, ON_THE_TOP_EDGE),
            ] {
                let answer = pick(r, &asking(serial, picking));
                assert_eq!((answer.frame, answer.rect), (serial, picking));
            }
            // Two frames asking, neither answer taken till after both: the newer's answer,
            // with its own serial (3), not the older's (7).
            draw(r, &asking(7, ON_THE_FACE));
            draw(r, &asking(3, ON_THE_TOP_EDGE));
            draw(r, &frame(&camera, &[1], &[], None));
            let answer = r.take_picking().unwrap();
            assert_eq!((answer.frame, answer.rect), (3, ON_THE_TOP_EDGE));
            assert_eq!(answer.hit.map(|h| h.hit), Some(Element::Edge(3)));
            // A refused frame asks nothing: no answer carries its serial.
            let mut broken = camera.clone();
            broken.eye = broken.target;
            let refused = Frame {
                serial: 99,
                ..frame(&broken, &[1], &[], Some(ON_THE_FACE))
            };
            assert!(r.prepare(&refused).is_err());
            for _ in 0..3 {
                draw(r, &frame(&camera, &[1], &[], None));
                assert_eq!(r.take_picking(), None);
            }
        },
    );
}

#[test]
fn a_picking_instance_is_a_face_from_1_or_an_edge_from_1_with_the_edge_bit() {
    use super::EDGE_BIT;
    let edge_bit = u64::from(EDGE_BIT);
    for (instance, want) in [
        (0, None),
        (1, Some(Element::Face(1))),
        (edge_bit - 1, Some(Element::Face(EDGE_BIT - 1))),
        (edge_bit, None),
        (edge_bit + 1, Some(Element::Edge(1))),
        (u64::from(u32::MAX), Some(Element::Edge(EDGE_BIT - 1))),
        (1 << 32, None),
        (u64::MAX, None),
    ] {
        assert_eq!(Element::of_instance(instance), want, "{instance:#x}");
    }
}

/// A picking request of `rect` in a frame `size`, asked of no body in particular.
fn request(rect: PixelRect, size: [u32; 2]) -> PendingPick {
    PendingPick {
        serial: 0,
        bodies: vec![],
        rect,
        size,
        frame: front().frame().unwrap(),
        origin: [0.0; 3],
        depth: 1.0,
        projection: ViewProjection::Orthographic {
            camera_mode: OrthographicCameraMode::NearPlaneCenter,
            vertical_world_size: 44.0,
            far_plane_distance: 100.0,
        },
        perspective: false,
        span: 44.0,
        strip_fraction: super::edge_fraction(size[1]),
    }
}

/// Body 1's face `n`'s picking ID.
fn face_id(n: u64) -> re_renderer::PickingLayerId {
    re_renderer::PickingLayerId {
        object: re_renderer::PickingLayerObjectId(1),
        instance: re_renderer::PickingLayerInstanceId(n),
    }
}

/// What `request` finds in `ids`, `extent` in size, every body current: its element and
/// pixel.
fn nearest_of(
    request: &PendingPick,
    ids: &[re_renderer::PickingLayerId],
    extent: [u32; 2],
) -> Option<(Element, [u32; 2])> {
    let [any, _] = request.nearest(ids, glam::UVec2::from(extent), |_| true);
    any.map(|(_, pick)| (pick.hit, pick.pixel))
}

/// Body 1's edge `n`'s picking ID.
fn edge_id(n: u64) -> re_renderer::PickingLayerId {
    re_renderer::PickingLayerId {
        object: re_renderer::PickingLayerObjectId(1),
        instance: re_renderer::PickingLayerInstanceId(u64::from(silico_protocol::EDGE_BIT) | n),
    }
}

#[test]
fn the_edge_nearest_the_centre_is_found_beside_the_nearest_of_anything() {
    let size = [WIDTH, HEIGHT];
    let empty = re_renderer::PickingLayerId::default();
    // A 5 by 1 row from column 10: the centre (2.5, 0.5) is the middle pixel's, a face's;
    // Edge4 two pixels left of it and Edge7 one right of it. The nearest of anything is
    // the face, at (12, 10); the nearest edge Edge7, at (13, 10), one from the centre, not
    // Edge4, two from it.
    let row = request(rect([10, 10], [5, 1]), size);
    let ids = [empty, empty, face_id(2), edge_id(7), empty];
    let ids_far = [edge_id(4), empty, face_id(2), empty, empty];
    let [any, edge] = row.nearest(&ids, glam::UVec2::new(5, 1), |_| true);
    let seen = |found: Option<(u32, Pick)>| found.map(|(i, pick)| (i, pick.hit, pick.pixel));
    assert_eq!(seen(any), Some((2, Element::Face(2), [12, 10])));
    assert_eq!(seen(edge), Some((3, Element::Edge(7), [13, 10])));
    let [_, edge] = row.nearest(&ids_far, glam::UVec2::new(5, 1), |_| true);
    assert_eq!(seen(edge), Some((0, Element::Edge(4), [10, 10])));
    // No edge in it: none; and an edge of a body not current is passed over, as `hit`'s
    // are.
    let [any, edge] = row.nearest(
        &[empty, empty, face_id(2), empty, empty],
        glam::UVec2::new(5, 1),
        |_| true,
    );
    assert_eq!(seen(any), Some((2, Element::Face(2), [12, 10])));
    assert_eq!(seen(edge), None);
    let [any, edge] = row.nearest(&ids, glam::UVec2::new(5, 1), |_| false);
    assert_eq!((seen(any), seen(edge)), (None, None));
}

#[test]
fn a_picking_answers_the_edge_in_its_band_beside_the_face_under_its_centre() {
    on_lavapipe(
        "render::frame_tests::a_picking_answers_the_edge_in_its_band_beside_the_face_under_its_centre",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // Centred on (32, 23), on the front face (rows 20.73 to 31.64), 2.5 rows under
            // its top edge's line (row 20.73): a 7 by 7 square, rows 20 to 26, reaches row
            // 20, where Edge3 is drawn (1 point either side of its line). Its hit is the
            // face under the centre; its edge, Edge3, at row 20 or 21, column 32.
            let band = rect([29, 20], [7, 7]);
            let picking = pick(r, &frame(&camera, &[1], &[], Some(band)));
            let hit = picking.hit.expect("the face under the centre");
            assert_eq!((hit.hit, hit.pixel), (Element::Face(3), [32, 23]));
            let edge = picking.edge.expect("the top edge in the band");
            assert_eq!(edge.hit, Element::Edge(3));
            assert_eq!(edge.pixel[0], 32);
            assert!((20..=21).contains(&edge.pixel[1]), "{:?}", edge.pixel);
            // Its point at its pixel's centre: on the front, y = 0 (to the edges' depth offset
            // and f32's depth), and within the row's 11/12 mm of the edge's z = 10.
            let point = edge.point.expect("its point");
            assert!(
                point[1].abs() < 0.05 && (point[2] - 10.0).abs() < 1.0,
                "{point:?}"
            );
            // A 3 by 3 square on the same centre, rows 22 to 24, doesn't reach it: no edge.
            let narrow = pick(r, &frame(&camera, &[1], &[], Some(rect([31, 22], [3, 3]))));
            assert_eq!(narrow.edge, None);
            assert_eq!(narrow.hit.map(|h| h.hit), Some(Element::Face(3)));
        },
    );
}

#[test]
fn a_hover_is_checked_as_highlights_are_and_changes_no_picking_id_and_uploads_nothing() {
    on_lavapipe(
        "render::frame_tests::a_hover_is_checked_as_highlights_are_and_changes_no_picking_id_and_uploads_nothing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            draw(r, &frame(&camera, &[1], &[], None));
            let uploaded = r.uploads();
            // The front face and its top edge hovered, one of them highlighted too: what
            // each pick finds is what it finds of the plain frame, and nothing is uploaded.
            let hovered = [(1, Element::Face(3)), (1, Element::Edge(3))];
            let lit = [(1, Element::Face(3))];
            for picking in [ON_THE_FACE, ON_THE_TOP_EDGE] {
                let plain = hit(r, &frame(&camera, &[1], &[], Some(picking)));
                let both = Frame {
                    hovered: &hovered,
                    outlines: Outlines {
                        selected: [1, 2, 3],
                        hovered: [4, 5, 6],
                    },
                    ..frame(&camera, &[1], &lit, Some(picking))
                };
                assert!(plain.is_some());
                assert_eq!(hit(r, &both), plain);
            }
            assert_eq!(r.uploads(), uploaded);
            // Refused as a highlight is: a body not drawn, an element its mesh hasn't.
            let not_drawn = [(2, Element::Face(1))];
            let mut refused_by = |hovered: &[(u32, Element)]| {
                let frame = Frame {
                    hovered,
                    ..frame(&camera, &[1], &[], None)
                };
                match r.prepare(&frame) {
                    Err(RenderFailure::Invalid(invalid)) => (invalid.parameter, invalid.message),
                    other => panic!("not refused: {:?}", other.err()),
                }
            };
            assert_eq!(
                refused_by(&not_drawn),
                (
                    "hovered",
                    "hovered[0]'s body 2 isn't drawn in the frame".to_owned()
                )
            );
            assert_eq!(
                refused_by(&[(1, Element::Face(1)), (1, Element::Edge(13))]),
                (
                    "hovered",
                    "hovered[1], Edge(13) of body 1, isn't one of its 12 edges (numbered from 1)"
                        .to_owned()
                )
            );
        },
    );
}

#[test]
fn of_pixels_as_near_the_first_in_rows_is_taken_and_one_past_the_frames_edge_never() {
    let size = [WIDTH, HEIGHT];
    // Two pixels beside each other, the centre (1, 0.5) between them, each 0.5 away: the
    // first, Face1 at (10, 10). And above each other: the first row's.
    let across = request(rect([10, 10], [2, 1]), size);
    assert_eq!(
        nearest_of(&across, &[face_id(1), face_id(2)], [2, 1]),
        Some((Element::Face(1), [10, 10]))
    );
    let down = request(rect([10, 10], [1, 2]), size);
    assert_eq!(
        nearest_of(&down, &[face_id(1), face_id(2)], [1, 2]),
        Some((Element::Face(1), [10, 10]))
    );
    // Three pixels from column 63, the frame's last: the middle one (column 64) is at the
    // centre, but past the frame; the nearest of the frame's own is column 63, one away.
    // So for row 47, the frame's last.
    let right = request(rect([63, 10], [3, 1]), size);
    let ids = [face_id(1), face_id(2), face_id(3)];
    assert_eq!(
        nearest_of(&right, &ids, [3, 1]),
        Some((Element::Face(1), [63, 10]))
    );
    let bottom = request(rect([10, 47], [1, 3]), size);
    assert_eq!(
        nearest_of(&bottom, &ids, [1, 3]),
        Some((Element::Face(1), [10, 47]))
    );
    // A readback not of the rectangle's size (one ID for two pixels), or of no width:
    // nothing.
    assert_eq!(nearest_of(&across, &[face_id(1)], [2, 1]), None);
    assert_eq!(nearest_of(&across, &[], [0, 1]), None);
}

#[test]
fn of_edges_as_near_the_first_in_rows_is_the_edge_taken() {
    // A 3 by 1 row from column 10, its centre (1.5, 0.5): Edge4 at its first pixel and
    // Edge7 at its last, each 1 from the centre, a face between them. The edge is Edge4,
    // the first; so for a 1 by 3 column, the first row's.
    let size = [WIDTH, HEIGHT];
    let seen = |found: Option<(u32, Pick)>| found.map(|(i, pick)| (i, pick.hit, pick.pixel));
    let row = request(rect([10, 10], [3, 1]), size);
    let ids = [edge_id(4), face_id(2), edge_id(7)];
    let [any, edge] = row.nearest(&ids, glam::UVec2::new(3, 1), |_| true);
    assert_eq!(seen(any), Some((1, Element::Face(2), [11, 10])));
    assert_eq!(seen(edge), Some((0, Element::Edge(4), [10, 10])));
    let column = request(rect([10, 10], [1, 3]), size);
    let [_, edge] = column.nearest(&ids, glam::UVec2::new(1, 3), |_| true);
    assert_eq!(seen(edge), Some((0, Element::Edge(4), [10, 10])));
}

#[test]
fn a_frame_the_gpu_failed_drops_its_request_and_every_gpu_mesh_but_keeps_the_bodies() {
    on_lavapipe(
        "render::frame_tests::a_frame_the_gpu_failed_drops_its_request_and_every_gpu_mesh_but_keeps_the_bodies",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            draw(r, &frame(&camera, &[1], &[], None));
            assert_eq!(r.uploads(), 6);
            // A frame that asks, then fails as the GPU fails one (its request the last
            // readback identifier given).
            draw(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE)));
            let id = r.readback - 1;
            assert!(r.pending.contains_key(&id));
            r.failed(Some(id));
            assert!(r.pending.is_empty());
            // The next frame is refused, backing off ([`Backoff`]); the one after it uploads
            // all six faces again, and no answer comes.
            assert!(matches!(
                r.prepare(&frame(&camera, &[1], &[], None)),
                Err(RenderFailure::BackingOff { frames: 1 })
            ));
            assert_eq!(r.uploads(), 6);
            draw(r, &frame(&camera, &[1], &[], None));
            assert_eq!(r.uploads(), 12);
            assert_eq!(r.take_picking(), None);
            // The body is still drawn, and picked.
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))),
                Some((1, Element::Face(3), [32, 26]))
            );
        },
    );
}

/// What one frame did: drawn, failed as the GPU fails one, or refused backing off, with the
/// frames refused from it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Drawn,
    Failed,
    Refused(u32),
}

/// `frame` prepared, and submitted if it was built, and what it did.
fn outcome(r: &mut Renderer, frame: &Frame<'_>) -> Outcome {
    match r.prepare(frame) {
        Ok(commands) => {
            r.ctx.queue.submit([commands]);
            wait(r);
            Outcome::Drawn
        }
        Err(RenderFailure::Failed(_)) => Outcome::Failed,
        Err(RenderFailure::BackingOff { frames }) => Outcome::Refused(frames),
        Err(other) => panic!("{other}"),
    }
}

#[test]
fn after_the_gpu_fails_a_frame_the_next_back_off_doubling_until_one_succeeds() {
    on_lavapipe(
        "render::frame_tests::after_the_gpu_fails_a_frame_the_next_back_off_doubling_until_one_succeeds",
        |r| {
            use Outcome::{Drawn, Failed, Refused};
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let plain = frame(&camera, &[1], &[], None);
            assert_eq!(outcome(r, &plain), Drawn);
            assert_eq!(r.uploads(), 6);
            // Four tries in a row fail: after each, 1, 2, 4, then 8 frames refused at once,
            // each uploading nothing; each try after the first uploads the six faces again
            // (a failure drops them); the fifth try draws.
            r.fail_frames = 4;
            let mut want = vec![];
            for refused in [1, 2, 4, 8] {
                want.push((Failed, if want.is_empty() { 0 } else { 6 }));
                want.extend((1..=refused).rev().map(|left| (Refused(left), 0)));
            }
            want.push((Drawn, 6));
            let got: Vec<(Outcome, u64)> = want
                .iter()
                .map(|_| {
                    let before = r.uploads();
                    let done = outcome(r, &plain);
                    (done, r.uploads() - before)
                })
                .collect();
            assert_eq!(got, want);
            // A success ended it: the next failure refuses one frame again, not 16.
            r.fail_frames = 1;
            let got: Vec<Outcome> = (0..3).map(|_| outcome(r, &plain)).collect();
            assert_eq!(got, [Failed, Refused(1), Drawn]);
        },
    );
}

#[test]
fn a_frame_refused_backing_off_asks_nothing_is_checked_for_nothing_and_caps_at_64() {
    on_lavapipe(
        "render::frame_tests::a_frame_refused_backing_off_asks_nothing_is_checked_for_nothing_and_caps_at_64",
        |r| {
            use Outcome::{Drawn, Failed, Refused};
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let plain = frame(&camera, &[1], &[], None);
            r.fail_frames = 1;
            assert_eq!(outcome(r, &plain), Failed);
            // Refused before its checks (body 2 isn't uploaded) and asking for no picking.
            let readback = r.readback;
            assert_eq!(
                outcome(r, &frame(&camera, &[2], &[], Some(ON_THE_FACE))),
                Refused(1)
            );
            assert_eq!((r.readback, r.pending.len()), (readback, 0));
            assert_eq!(outcome(r, &plain), Drawn);
            assert_eq!(
                RenderFailure::BackingOff { frames: 3 }.to_string(),
                "the GPU failed a recent frame: backing off, 3 frame(s) refused before the next try"
            );
            // Nine failures in a row: the runs refused after them, 1, 2, 4, … doubling to 64,
            // and no further.
            r.fail_frames = 9;
            let mut runs: Vec<u32> = vec![];
            loop {
                match outcome(r, &plain) {
                    Failed => runs.push(0),
                    Refused(_) => *runs.last_mut().unwrap() += 1,
                    Drawn => break,
                }
            }
            assert_eq!(runs, [1, 2, 4, 8, 16, 32, 64, 64, 64]);
        },
    );
}

#[test]
fn a_new_mesh_has_the_next_frame_tried_but_keeps_the_doubling() {
    on_lavapipe(
        "render::frame_tests::a_new_mesh_has_the_next_frame_tried_but_keeps_the_doubling",
        |r| {
            use Outcome::{Failed, Refused};
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let plain = frame(&camera, &[1], &[], None);
            // Fail, refuse one, fail (refusing two), a new mesh (the next frame tried), fail:
            // four refused, not one, as an edit made during a shortage would otherwise have
            // every other frame upload the model.
            r.fail_frames = 3;
            assert_eq!(outcome(r, &plain), Failed);
            assert_eq!(outcome(r, &plain), Refused(1));
            assert_eq!(outcome(r, &plain), Failed);
            r.upload(1, &Arc::new(box_mesh([0.0; 3], [40.0, 20.0, 11.0])))
                .unwrap();
            assert_eq!(outcome(r, &plain), Failed);
            assert_eq!(outcome(r, &plain), Refused(4));
        },
    );
}

#[test]
fn forgetting_a_body_leaves_a_backoff_as_it_was() {
    on_lavapipe(
        "render::frame_tests::forgetting_a_body_leaves_a_backoff_as_it_was",
        |r| {
            use Outcome::{Drawn, Failed, Refused};
            r.upload(1, &the_box()).unwrap();
            r.upload(
                2,
                &Arc::new(box_mesh([0.0, -30.0, 0.0], [40.0, 20.0, 10.0])),
            )
            .unwrap();
            let camera = front();
            r.fail_frames = 2;
            assert_eq!(outcome(r, &frame(&camera, &[1, 2], &[], None)), Failed);
            assert_eq!(outcome(r, &frame(&camera, &[1, 2], &[], None)), Refused(1));
            assert_eq!(outcome(r, &frame(&camera, &[1, 2], &[], None)), Failed);
            // Refusing two; forgetting body 2 (and one never uploaded) refuses them still.
            r.forget(2);
            r.forget(9);
            let one = frame(&camera, &[1], &[], None);
            assert_eq!(outcome(r, &one), Refused(2));
            assert_eq!(outcome(r, &one), Refused(1));
            assert_eq!(outcome(r, &one), Drawn);
        },
    );
}

#[test]
fn a_new_mesh_ends_a_backoff_and_the_same_one_again_doesnt() {
    on_lavapipe(
        "render::frame_tests::a_new_mesh_ends_a_backoff_and_the_same_one_again_doesnt",
        |r| {
            use Outcome::{Drawn, Failed, Refused};
            let held = the_box();
            r.upload(1, &held).unwrap();
            let camera = front();
            let plain = frame(&camera, &[1], &[], None);
            r.fail_frames = 2;
            assert_eq!(outcome(r, &plain), Failed);
            assert_eq!(outcome(r, &plain), Refused(1));
            assert_eq!(outcome(r, &plain), Failed);
            // Refusing two now. The same mesh again changes nothing, so the backoff stays.
            r.upload(1, &held).unwrap();
            r.upload(1, &the_box()).unwrap();
            assert_eq!(outcome(r, &plain), Refused(2));
            // Another mesh ends it: the next frame is tried, and draws.
            r.upload(1, &Arc::new(box_mesh([0.0; 3], [40.0, 20.0, 11.0])))
                .unwrap();
            assert_eq!(outcome(r, &plain), Drawn);
        },
    );
}

#[test]
fn an_orthographic_view_is_drawn_down_to_the_least_height_its_bodies_reach_allows() {
    on_lavapipe(
        "render::frame_tests::an_orthographic_view_is_drawn_down_to_the_least_height_its_bodies_reach_allows",
        |r| {
            r.upload(1, &the_box()).unwrap();
            // The box's reach from the target, its farthest corner: from (21, 10, 7), the
            // corner (0, 0, 0) on x, y, and z alike (21, 10, 7 away), √590; from (5, 4, 3),
            // the corner (40, 20, 10) (35, 16, 7 away), √1530. The least height at 48 rows is
            // 48 times that over 2^19, as a scene's check takes it (0.00222 and 0.00358 mm):
            // drawn at it, refused just under it.
            for (target, squared) in [([21.0, 10.0, 7.0], 590.0f64), ([5.0, 4.0, 3.0], 1530.0)] {
                let least = 48.0 * squared.sqrt() / 524_288.0;
                let at = |view_height: f64| Camera {
                    projection: Projection::Orthographic { view_height },
                    eye: [target[0], target[1] - 60.0, target[2]],
                    target,
                    up: [0.0, 0.0, 1.0],
                };
                let camera = at(least);
                r.prepare(&frame(&camera, &[1], &[], None)).unwrap();
                let camera = at(least.next_down());
                assert_eq!(
                    refused(r, &frame(&camera, &[1], &[], None)),
                    "camera.view_height",
                    "{target:?}"
                );
            }
        },
    );
}

#[test]
fn a_perspective_frame_takes_the_angles_a_scenes_check_does_at_its_size_and_any_eye() {
    on_lavapipe(
        "render::frame_tests::a_perspective_frame_takes_the_angles_a_scenes_check_does_at_its_size_and_any_eye",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let at = |distance: f64, field_of_view: f64| Camera {
                projection: Projection::Perspective { field_of_view },
                eye: [21.0, -distance, 7.0],
                target: [21.0, 10.0, 7.0],
                up: [0.0, 0.0, 1.0],
            };
            // Any eye: 0.1 mm before the front face (its corners 221 times as far across the
            // view as along it at 1°, past the pyramid's 190.6 over 48 rows: cut), in its
            // plane, or inside the box: each drawn, what's outside the pyramid cut away.
            for (distance, field_of_view) in [(0.2, 1.0), (0.1, 1.0), (0.0, 60.0), (-1.0, 60.0)] {
                let camera = at(distance, field_of_view);
                r.prepare(&frame(&camera, &[1], &[], None)).unwrap();
            }
            // The angle at the frame's size: at 64 by 48, from 0.005246° (by hand, the
            // quadratic's root): 0.001° is refused, 0.006° drawn.
            let camera = at(1e5, 0.001);
            assert_eq!(
                refused(r, &frame(&camera, &[1], &[], None)),
                "camera.field_of_view"
            );
            let camera = at(1e5, 0.006);
            r.prepare(&frame(&camera, &[1], &[], None)).unwrap();
        },
    );
}

/// Every pixel's ID of `camera`'s frame of `bodies`, `size` pixels (at most 256 a side), as
/// its picking reads them back: the body's number and the element's, as the worker's ID
/// buffer writes them.
fn frame_ids(r: &mut Renderer, camera: &Camera, bodies: &[u32], size: [u32; 2]) -> Vec<[u32; 2]> {
    frame_ids_at(r, camera, bodies, size, 1.0)
}

/// [`frame_ids`] at `pixels_per_point`.
fn frame_ids_at(
    r: &mut Renderer,
    camera: &Camera,
    bodies: &[u32],
    size: [u32; 2],
    pixels_per_point: f32,
) -> Vec<[u32; 2]> {
    let asking = Frame {
        size_px: size,
        picking: Some(rect([0, 0], size)),
        pixels_per_point,
        ..frame(camera, bodies, &[], None)
    };
    pick(r, &asking);
    // `pick` took the answer's hit; the frame again, and its readback read whole.
    draw(r, &asking);
    draw(
        r,
        &Frame {
            serial: asking.serial.wrapping_add(1),
            picking: None,
            ..asking
        },
    );
    r.harvest();
    let answer = r.picked.take().expect("no answer");
    answer
        .ids
        .iter()
        .map(|id| {
            [
                u32::try_from(id.object.0).unwrap(),
                u32::try_from(id.instance.0).unwrap(),
            ]
        })
        .collect()
}

#[test]
fn an_edge_past_the_frames_side_by_less_than_its_stroke_at_two_pixels_a_point_is_drawn() {
    on_lavapipe(
        "render::frame_tests::an_edge_past_the_frames_side_by_less_than_its_stroke_at_two_pixels_a_point_is_drawn",
        |r| {
            // Orthographic about the origin from −y, 48 mm over 48 rows, 64 wide: a pixel a
            // millimetre, the frame's right side at x = 32. At 2 pixels a point an edge's
            // 2-point stroke is 4 pixels, reaching 2 past its line: one along z at x = 33.25,
            // 1.25 past the side, reaches the last column's centres (1.75 from it), by hand
            // rows 16 to 32; at one pixel a point (a reach of 1) it doesn't reach the frame.
            let edge = Arc::new(Mesh {
                positions: vec![],
                normals: vec![],
                triangles: vec![],
                face_triangles: vec![],
                edge_points: vec![[33.25, 0.0, -10.0], [33.25, 0.0, 10.0]],
                edge_point_counts: vec![2],
                vertex_points: vec![],
                valid: None,
            });
            r.upload(1, &edge).unwrap();
            let camera = Camera {
                projection: Projection::Orthographic { view_height: 48.0 },
                eye: [0.0, -10.0, 0.0],
                target: [0.0; 3],
                up: [0.0, 0.0, 1.0],
            };
            let id = [1, 0x8000_0001];
            let ids = frame_ids_at(r, &camera, &[1], [64, 48], 2.0);
            for row in 16..=32 {
                assert_eq!(ids[row * 64 + 63], id, "row {row}");
            }
            let ids = frame_ids_at(r, &camera, &[1], [64, 48], 1.0);
            assert!(ids.iter().all(|id| *id == [0, 0]));
        },
    );
}

#[test]
fn a_box_seen_from_inside_draws_in_a_frame_as_the_workers_render_draws_it() {
    on_lavapipe(
        "render::frame_tests::a_box_seen_from_inside_draws_in_a_frame_as_the_workers_render_draws_it",
        |r| {
            // The bench's box with the eye at its centre, in a wall's plane (x = 0), and 1 mm
            // from it, looking along +y at 90°: its walls beside and behind the eye cut to the
            // view's pyramid, as a frame and the worker's render cut them, pixel for pixel.
            for eye in [[20.0, 10.0, 5.0], [0.0, 10.0, 5.0], [1.0, 10.0, 5.0]] {
                let camera = Camera {
                    projection: Projection::Perspective {
                        field_of_view: 90.0,
                    },
                    eye,
                    target: [eye[0], 20.0, eye[2]],
                    up: [0.0, 0.0, 1.0],
                };
                let scene = silico_protocol::RenderScene {
                    bodies: vec![silico_protocol::RenderBody {
                        body: 1,
                        mesh: Mesh::clone(&the_box()),
                        highlight_faces: vec![],
                        highlight_edges: vec![],
                    }],
                    camera: camera.clone(),
                    width: WIDTH,
                    height: HEIGHT,
                    ids: true,
                };
                scene.check().unwrap();
                let rendered = r.render(&scene).unwrap().ids.unwrap();
                assert!(
                    rendered.contains(&[1, 4]),
                    "{eye:?}: the far wall isn't drawn"
                );
                r.upload(1, &the_box()).unwrap();
                let framed = frame_ids(r, &camera, &[1], [WIDTH, HEIGHT]);
                // Pixel for pixel, but where the frame's picking takes an edge the render's
                // faces hide (N5): a frame that picks draws each edge again toward the eye by
                // its reach (at 48 rows a vertex's, 1.2% of 2 tan 45° of its depth), and every
                // edge of a box seen from inside is concave, the faces beside it rising toward
                // the eye away from its line. Such a pixel is the edge's in the frame, one of
                // the two faces it bounds (whose planes hold both its ends) in the render, and
                // within its stroke, a point of its drawn segment (the render draws no edge
                // there, so no pixel the render gives an edge or the background differs).
                let mesh = the_box();
                let high = [40.0, 20.0, 10.0];
                let bounds = |edge: u32, face: u32| {
                    let e = usize::try_from(edge - 1).unwrap();
                    let (a, b) = (mesh.edge_points[2 * e], mesh.edge_points[2 * e + 1]);
                    let k = usize::try_from((face - 1) / 2).unwrap();
                    let at = if face % 2 == 1 { 0.0 } else { high[k] };
                    a[k] == at && b[k] == at
                };
                let mut differ = 0;
                for (i, (&got, &want)) in framed.iter().zip(&rendered).enumerate() {
                    if got == want {
                        continue;
                    }
                    differ += 1;
                    let ([1, picked], [1, face]) = (got, want) else {
                        panic!(
                            "{eye:?}: pixel {i} is {got:?} in the frame, {want:?} in the render"
                        );
                    };
                    let edge = picked & !silico_protocol::EDGE_BIT;
                    assert!(
                        picked & silico_protocol::EDGE_BIT != 0
                            && face & silico_protocol::EDGE_BIT == 0
                            && bounds(edge, face),
                        "{eye:?}: pixel {i} is {got:?} in the frame, {want:?} in the render"
                    );
                    let e = usize::try_from(edge - 1).unwrap();
                    let ends = (mesh.edge_points[2 * e], mesh.edge_points[2 * e + 1]);
                    let [Some(a), Some(b)] =
                        super::project_segment(&camera, [WIDTH, HEIGHT], ends.0, ends.1).unwrap()
                    else {
                        panic!("{eye:?}: Edge{edge} isn't drawn whole");
                    };
                    let off = from_segment(i, a, b);
                    assert!(off <= 1.0, "{eye:?}: pixel {i} is {off} off Edge{edge}");
                }
                assert!(differ < framed.len() / 4, "{eye:?}: {differ} pixels differ");
            }
        },
    );
}

#[test]
fn edges_and_outlines_are_as_wide_as_their_points_at_the_frames_pixels_per_point() {
    on_lavapipe(
        "render::frame_tests::edges_and_outlines_are_as_wide_as_their_points_at_the_frames_pixels_per_point",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let at = |ppp: f32, lit: &'static [(u32, Element)], picking| Frame {
                pixels_per_point: ppp,
                ..frame(&camera, &[1], lit, picking)
            };
            // Row 19's centre is 1.23 pixels from the top edge's line: past its 1-point half
            // width at 1 pixel a point, inside it at 2.
            let beside = Some(rect([32, 19], [1, 1]));
            assert_eq!(hit(r, &at(1.0, &[], beside)), None);
            assert_eq!(
                hit(r, &at(2.0, &[], beside)),
                Some((1, Element::Edge(3), [32, 19]))
            );
            // The front face's outline, 1.5 points: twice as wide at 2 pixels a point, so
            // it changes more pixels round the face than at 1.
            const LIT: &[(u32, Element)] = &[(1, Element::Face(3))];
            let mut changed = |ppp: f32| {
                let commands = r.prepare(&at(ppp, &[], None)).unwrap();
                let plain = composited(r, Some(commands));
                let commands = r.prepare(&at(ppp, LIT, None)).unwrap();
                let lit = composited(r, Some(commands));
                plain.iter().zip(&lit).filter(|(a, b)| a != b).count()
            };
            let (one, two) = (changed(1.0), changed(2.0));
            assert!(two * 2 > one * 3, "{one} pixels changed at 1, {two} at 2");
        },
    );
}

#[test]
fn a_hover_and_a_highlight_are_outlined_each_in_the_callers_colour() {
    // The front face (Face3, columns 9.09 to 52.73, rows 20.73 to 31.64) hovered, then
    // highlighted, outlined in colours picked by hand, unlike the defaults (white for the
    // hover, the highlight's blue): magenta for the hover, green for the selection. Just
    // outside the face, (8, 26) left of its left side and (32, 20) above its top, within
    // the outline's 1.5 points, the plain frame draws the edges' dark line; outlined,
    // each pixel is the outline's colour, the caller's for that layer, and the face's
    // inside, (32, 26), is as uploaded.
    on_lavapipe(
        "render::frame_tests::a_hover_and_a_highlight_are_outlined_each_in_the_callers_colour",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            const MAGENTA: [u8; 3] = [255, 0, 255];
            const GREEN: [u8; 3] = [0, 255, 0];
            assert_ne!(Outlines::DEFAULT.hovered, MAGENTA);
            assert_ne!(Outlines::DEFAULT.selected, GREEN);
            let outlines = Outlines {
                selected: GREEN,
                hovered: MAGENTA,
            };
            let face = [(1, Element::Face(3))];
            let mut image = |lit: &[(u32, Element)], hovered: &[(u32, Element)]| {
                let commands = r
                    .prepare(&Frame {
                        hovered,
                        outlines,
                        ..frame(&camera, &[1], lit, None)
                    })
                    .unwrap();
                composited(r, Some(commands))
            };
            let at = |pixels: &[[u8; 4]], x: u32, y: u32| pixels[(y * WIDTH + x) as usize];
            let near = |got: [u8; 4], [r, g, b]: [u8; 3]| {
                got.iter()
                    .zip([r, g, b, 255])
                    .all(|(got, want)| got.abs_diff(want) <= 2)
            };
            let plain = image(&[], &[]);
            let hovered = image(&[], &face);
            let lit = image(&face, &[]);
            for (x, y) in [(8, 26), (32, 20)] {
                let [r, g, b, _] = at(&plain, x, y);
                assert!(
                    r < 64 && g < 64 && b < 64,
                    "plain ({x}, {y}): {:?}",
                    at(&plain, x, y)
                );
                assert!(
                    near(at(&hovered, x, y), MAGENTA),
                    "hovered ({x}, {y}): {:?}",
                    at(&hovered, x, y)
                );
                assert!(
                    near(at(&lit, x, y), GREEN),
                    "lit ({x}, {y}): {:?}",
                    at(&lit, x, y)
                );
            }
            assert_eq!(at(&hovered, 32, 26), at(&plain, 32, 26));
            assert_eq!(at(&lit, 32, 26), at(&plain, 32, 26));
            // Nothing hovered or lit: no pixel of either colour anywhere.
            assert!(!plain.iter().any(|&p| near(p, MAGENTA) || near(p, GREEN)));
            assert!(!hovered.iter().any(|&p| near(p, GREEN)));
            assert!(!lit.iter().any(|&p| near(p, MAGENTA)));
        },
    );
}

/// The composited image of the box from the front, `lit` highlighted and `hovered` hovered,
/// outlined in `outlines`.
fn outlined(
    r: &mut Renderer,
    lit: &[(u32, Element)],
    hovered: &[(u32, Element)],
    outlines: Outlines,
) -> Vec<[u8; 4]> {
    let camera = front();
    let commands = r
        .prepare(&Frame {
            hovered,
            outlines,
            ..frame(&camera, &[1], lit, None)
        })
        .unwrap();
    composited(r, Some(commands))
}

#[test]
fn a_hovered_edge_alone_is_outlined_and_a_frame_with_nothing_lit_makes_no_outline_pass() {
    // The front's top edge (Edge3, row 20.73) hovered alone: outlined in the hover's colour,
    // magenta, picked by hand unlike the defaults; nothing lit, no pixel of it. The outline
    // pass is made only for a frame with something to outline: its jump-flooding pipelines
    // (re_renderer's `OutlineMaskProcessor`) are first made for the hovered frame, none for
    // the plain one before it. And a body with every list lit (faces and edges, highlighted
    // and hovered) is outlined too: the selection's green round the front face.
    on_lavapipe(
        "render::frame_tests::a_hovered_edge_alone_is_outlined_and_a_frame_with_nothing_lit_makes_no_outline_pass",
        |r| {
            r.upload(1, &the_box()).unwrap();
            const MAGENTA: [u8; 3] = [255, 0, 255];
            const GREEN: [u8; 3] = [0, 255, 0];
            let outlines = Outlines {
                selected: GREEN,
                hovered: MAGENTA,
            };
            let near = |got: &[u8; 4], [r, g, b]: [u8; 3]| {
                got.iter()
                    .zip([r, g, b, 255])
                    .all(|(got, want)| got.abs_diff(want) <= 2)
            };
            let count =
                |pixels: &[[u8; 4]], colour| pixels.iter().filter(|p| near(p, colour)).count();
            let pipelines = |r: &Renderer| r.ctx.gpu_resources.statistics().num_render_pipelines;
            let plain = outlined(r, &[], &[], outlines);
            let before = pipelines(r);
            assert_eq!(count(&plain, MAGENTA), 0);
            let hovered = outlined(r, &[], &[(1, Element::Edge(3))], outlines);
            assert!(
                pipelines(r) > before,
                "no outline pass made for the hovered frame: {before} pipelines before it, {} after",
                pipelines(r)
            );
            assert!(
                count(&hovered, MAGENTA) > 0,
                "the hovered edge isn't outlined"
            );
            assert_eq!(count(&hovered, GREEN), 0);
            let every = outlined(
                r,
                &[(1, Element::Face(3)), (1, Element::Edge(3))],
                &[(1, Element::Face(4)), (1, Element::Edge(9))],
                outlines,
            );
            assert!(
                count(&every, GREEN) > 0,
                "the highlighted face isn't outlined"
            );
        },
    );
}

// ---- The tint (the app's feature hovered in its timeline) ----

/// The prototype's tint for the faces a feature made, typed from its own sources, not the
/// app's theme: DESIGN.md's "a feature's faces on hover: Blue.800 tint 40%", Blue.800 being
/// `#d4dbff` in re_ui 0.38.1's `color_table.ron`, and viewport.js's `MAKER`, `#d4dbff`,
/// mixed in at `uTintAmt = 0.4`.
const FEATURE_TINT: Tint = Tint {
    colour: [0xd4, 0xdb, 0xff],
    amount: 0.4,
};

/// An sRGB channel in linear light, by the sRGB formula.
fn linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        libm::pow((c + 0.055) / 1.055, 2.4)
    }
}

/// A linear channel in sRGB, 0 to 255, unrounded.
fn srgb_of(l: f64) -> f64 {
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * libm::pow(l, 1.0 / 2.4) - 0.055
    };
    c * 255.0
}

/// What a face lit by `light` shows, the body's colour mixed with `tint` at `amount` in
/// linear light first, by hand: `light · ((1 − amount) · body + amount · tint)`, in sRGB.
fn tinted_by_hand(light: f64, tint: [u8; 3], amount: f64) -> [f64; 3] {
    std::array::from_fn(|k| {
        srgb_of(light * ((1.0 - amount) * linear(BODY[k]) + amount * linear(tint[k])))
    })
}

/// The centre of pixel `i` of a [`WIDTH`]-wide image, in pixels.
fn pixel_centre(i: usize) -> (f64, f64) {
    let i = u32::try_from(i).unwrap();
    (f64::from(i % WIDTH) + 0.5, f64::from(i / WIDTH) + 0.5)
}

/// How far inside the box's front face, seen from [`front`], pixel `i`'s centre is, in
/// pixels: below 0 outside it. The face: columns 9.09 to 52.73, rows 20.73 to 31.64.
fn into_the_front(i: usize) -> f64 {
    let (x, y) = pixel_centre(i);
    let (left, right, top, bottom) = (9.09, 52.73, 20.73, 31.64);
    (x - left).min(right - x).min(y - top).min(bottom - y)
}

#[track_caller]
fn assert_colour(got: [u8; 4], want: [f64; 3], within: f64, what: &str) {
    for k in 0..3 {
        assert!(
            (f64::from(got[k]) - want[k]).abs() <= within,
            "{what}: {got:?} against {want:?}, channel {k}"
        );
    }
    assert_eq!(got[3], 255, "{what}: {got:?}");
}

/// The composited image of `frame` with `tinted` tinted in `tint` (and anything else
/// `frame` gives).
fn tinted_image(
    r: &mut Renderer,
    frame: &Frame<'_>,
    tinted: &[(u32, Element)],
    tint: Tint,
) -> Vec<[u8; 4]> {
    let commands = r
        .prepare(&Frame {
            tinted,
            tint,
            ..*frame
        })
        .unwrap();
    composited(r, Some(commands))
}

#[test]
fn a_frames_values_are_checked_without_a_gpu_each_one_refused_named() {
    // `Frame::check`, the rules `prepare` refuses a frame by, on no GPU: body 1 held, the
    // bench's box (6 faces, 12 edges, 8 vertices), body 2 not. Each refusal's parameter and
    // message by hand.
    let mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
    let held = |body: u32| (body == 1).then_some(&mesh);
    let camera = front();
    let good = Frame {
        tinted: &[(1, Element::Face(3))],
        tint: FEATURE_TINT,
        ..frame(&camera, &[1], &[(1, Element::Edge(3))], Some(ON_THE_FACE))
    };
    assert_eq!(good.check(held), Ok(()));
    let refused = |frame: Frame<'_>| {
        let invalid = frame.check(held).unwrap_err();
        (invalid.parameter, invalid.message)
    };
    let said = |parameter: &'static str, message: &str| (parameter, message.to_owned());
    assert_eq!(
        refused(Frame {
            pixels_per_point: 0.0,
            ..good
        }),
        said(
            "pixels_per_point",
            "pixels_per_point 0.0 must be above 0 and at most 32"
        )
    );
    assert_eq!(
        refused(Frame {
            bodies: &[1, 1],
            ..good
        }),
        said(
            "bodies",
            "bodies[1], 1, must be above the one before it (1): ascending, each once, from 1"
        )
    );
    assert_eq!(
        refused(Frame {
            bodies: &[1, 2],
            ..good
        }),
        said("bodies", "bodies[1], 2, isn't uploaded")
    );
    assert_eq!(
        refused(Frame {
            tint: Tint {
                colour: [255; 3],
                amount: 1.5
            },
            ..good
        }),
        said("tint", "tint's amount 1.5 must be finite, from 0 to 1")
    );
    assert_eq!(
        refused(Frame {
            highlights: &[(1, Element::Face(7))],
            ..good
        }),
        said(
            "highlights",
            "highlights[0], Face(7) of body 1, isn't one of its 6 faces (numbered from 1)"
        )
    );
    assert_eq!(
        refused(Frame {
            tinted: &[(1, Element::Face(3)), (1, Element::Edge(3))],
            ..good
        }),
        said(
            "tinted",
            "tinted[1], Edge(3) of body 1, isn't a face: only faces are tinted"
        )
    );
    assert_eq!(
        refused(Frame {
            vertices: Some(0.0),
            ..good
        }),
        said(
            "vertices",
            "vertices 0.0, a radius in points, must be finite and above 0"
        )
    );
    assert_eq!(
        refused(Frame {
            picking: Some(rect([0, 0], [0, 1])),
            ..good
        }),
        said(
            "picking",
            "picking PixelRect { min: [0, 0], size: [0, 1] } must be from 1 to 256 pixels on each \
             side, its corner within 16777216 of the frame's"
        )
    );
}

#[test]
fn a_tinted_face_is_the_bodys_colour_mixed_with_the_tint_and_nothing_else_changes() {
    // From the front, the box's front face (Face3, y = 0) faces the eye: its normal (0, 0, 1)
    // in the camera's frame, lit 0.2 + 3/√14 = 1.0018, clamped at 1, so it shows its colour
    // as it is. Tinted with the prototype's Blue.800 at 0.4, by hand in linear light:
    // BODY (170, 175, 182) is (0.4020, 0.4287, 0.4678), #d4dbff (0.6584, 0.7084, 1), mixed
    // (0.5045, 0.5406, 0.6807), in sRGB (188.3, 194.2, 215.2). Every pixel inside the face
    // by 2 of its border is that (to 1 of 255: the 8-bit sRGB of the tint re_renderer adds,
    // and of the target); every pixel off it by 2 is the plain frame's exactly. Nothing is
    // uploaded, and a pick finds what it finds in the plain frame.
    on_lavapipe(
        "render::frame_tests::a_tinted_face_is_the_bodys_colour_mixed_with_the_tint_and_nothing_else_changes",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let plain_frame = frame(&camera, &[1], &[], None);
            let plain = tinted_image(r, &plain_frame, &[], FEATURE_TINT);
            let uploaded = r.uploads();
            let lit = tinted_image(r, &plain_frame, &[(1, Element::Face(3))], FEATURE_TINT);
            assert_eq!(r.uploads(), uploaded, "a tint uploaded a mesh");
            let want = tinted_by_hand(1.0, FEATURE_TINT.colour, 0.4);
            for (k, hand) in [188.3, 194.2, 215.2].into_iter().enumerate() {
                assert!((want[k] - hand).abs() < 0.1, "{want:?}");
            }
            let (mut inside, mut outside) = (0, 0);
            for i in 0..lit.len() {
                let (x, y) = pixel_centre(i);
                let into = into_the_front(i);
                if into >= 2.0 {
                    assert_colour(lit[i], want, 1.0, &format!("({x}, {y}) on the face"));
                    // Plain, the body's colour, lit straight on.
                    assert_colour(
                        plain[i],
                        BODY.map(f64::from),
                        1.0,
                        &format!("({x}, {y}) plain"),
                    );
                    inside += 1;
                } else if into <= -2.0 {
                    assert_eq!(lit[i], plain[i], "({x}, {y}) off the face");
                    outside += 1;
                }
            }
            assert!(inside > 200 && outside > 2000, "{inside} {outside}");
            // A pick of the face, and of its top edge, finds what it finds unlit: the tint
            // changes no picking ID.
            for picking in [ON_THE_FACE, ON_THE_TOP_EDGE] {
                let unlit = hit(r, &frame(&camera, &[1], &[], Some(picking)));
                let tinted = hit(
                    r,
                    &Frame {
                        tinted: &[(1, Element::Face(3))],
                        tint: FEATURE_TINT,
                        ..frame(&camera, &[1], &[], Some(picking))
                    },
                );
                assert!(unlit.is_some());
                assert_eq!(tinted, unlit);
            }
            // The tint gone: the plain frame again, and no upload all along.
            assert_eq!(tinted_image(r, &plain_frame, &[], FEATURE_TINT), plain);
            assert_eq!(r.uploads(), uploaded);
        },
    );
}

#[test]
fn faces_tinted_in_any_order_or_twice_and_of_two_bodies_are_each_tinted() {
    // The box (body 1) and a second one 12 mm below it (body 2, z = -12 to -2), each seen
    // from the front by its front face (Face3): body 1's from row 20.73 to 31.64, body 2's
    // from 24 + 9 × 12/11 = 33.82 to 24 + 19 × 12/11 = 44.73, both from column 9.09 to
    // 52.73. Tinted as given out of order and one face twice, body 1's faces 6, 3, 2, and 2
    // again (6 and 2 are edge on from the front), body 2's 3 between them: each front face's
    // centre row is the tint's mix by hand, as one tinted alone is. Body 1's visible face
    // comes after a higher one, so a search of its list as given, unsorted, misses it.
    on_lavapipe(
        "render::frame_tests::faces_tinted_in_any_order_or_twice_and_of_two_bodies_are_each_tinted",
        |r| {
            r.upload(1, &the_box()).unwrap();
            r.upload(
                2,
                &Arc::new(box_mesh([0.0, 0.0, -12.0], [40.0, 20.0, 10.0])),
            )
            .unwrap();
            let camera = front();
            let both = frame(&camera, &[1, 2], &[], None);
            let plain = tinted_image(r, &both, &[], FEATURE_TINT);
            let tinted = [
                (1, Element::Face(6)),
                (2, Element::Face(3)),
                (1, Element::Face(3)),
                (1, Element::Face(2)),
                (1, Element::Face(2)),
            ];
            let lit = tinted_image(r, &both, &tinted, FEATURE_TINT);
            let want = tinted_by_hand(1.0, FEATURE_TINT.colour, 0.4);
            for (body, row) in [(1, 26u32), (2, 39)] {
                for column in [14u32, 32, 48] {
                    let at = usize::try_from(row * WIDTH + column).unwrap();
                    assert_colour(
                        plain[at],
                        BODY.map(f64::from),
                        1.0,
                        &format!("body {body} plain at ({column}, {row})"),
                    );
                    assert_colour(
                        lit[at],
                        want,
                        1.0,
                        &format!("body {body} tinted at ({column}, {row})"),
                    );
                }
            }
        },
    );
}

#[test]
fn a_tinted_face_is_mixed_before_the_light_shades_it() {
    // Seen obliquely (from (1, −1, 1), z up: right (1, 1, 0)/√2, up (−1, 1, 2)/√6, back
    // (1, −1, 1)/√3), each face is lit by hand from its normal in the camera's frame:
    // 0.2 + n · (1, 2, 3)/√14 + 0.5 · max(0, n · (−1, −3, −5)/√35), clamped to 0..1.
    // Face2 (x = 40), n = (0.7071, −0.4082, 0.5774): 0.2 + 0.4337 = 0.6337. Face3 (y = 0),
    // n = (−0.7071, −0.4082, 0.5774): 0.2 + 0.0557 = 0.2557. Face6 (z = 10), n = (0,
    // 0.8165, 0.5774): 0.2 + 0.8994, clamped at 1. The second light faces none of them. Each
    // tinted face's centre shows its light times the mix, not the mix of its lit colour
    // (Face2: plain (138.1, 142.3, 148.1), tinted (153.3, 158.1, 175.5)), to 1 of 255; and
    // the plain frame its light times the body's, checking the light by hand.
    on_lavapipe(
        "render::frame_tests::a_tinted_face_is_mixed_before_the_light_shades_it",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = oblique();
            let plain_frame = frame(&camera, &[1], &[], None);
            let plain = tinted_image(r, &plain_frame, &[], FEATURE_TINT);
            let every = [
                (1, Element::Face(2)),
                (1, Element::Face(3)),
                (1, Element::Face(6)),
            ];
            let lit = tinted_image(r, &plain_frame, &every, FEATURE_TINT);
            let (r2, r3, r6) = (2f64.sqrt(), 3f64.sqrt(), 6f64.sqrt());
            let light = |n: [f64; 3]| {
                let seen = [
                    (n[0] + n[1]) / r2,
                    (-n[0] + n[1] + 2.0 * n[2]) / r6,
                    (n[0] - n[1] + n[2]) / r3,
                ];
                let along = |l: [f64; 3]| {
                    let length = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
                    (seen[0] * l[0] + seen[1] * l[1] + seen[2] * l[2]) / length
                };
                let first = along([1.0, 2.0, 3.0]).clamp(0.0, 1.0);
                let second = along([-1.0, -3.0, -5.0]).clamp(0.0, 1.0);
                (0.2 + first + 0.5 * second).clamp(0.0, 1.0)
            };
            let normals = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]];
            let lights = normals.map(light);
            for (want, hand) in lights.iter().zip([0.6337, 0.2557, 1.0]) {
                assert!((want - hand).abs() < 1e-4, "{lights:?}");
            }
            for ((number, corners), light) in seen_faces().into_iter().zip(lights) {
                let centre = oblique_px(std::array::from_fn(|k| {
                    corners.iter().map(|c| c[k]).sum::<f64>() / 4.0
                }));
                let at = usize::try_from(centre[1] as u32 * WIDTH + centre[0] as u32).unwrap();
                assert_colour(
                    plain[at],
                    BODY.map(|c| srgb_of(light * linear(c))),
                    1.0,
                    &format!("Face{number} plain"),
                );
                assert_colour(
                    lit[at],
                    tinted_by_hand(light, FEATURE_TINT.colour, 0.4),
                    1.0,
                    &format!("Face{number} tinted"),
                );
            }
            let face2 = tinted_by_hand(lights[0], FEATURE_TINT.colour, 0.4);
            for (k, hand) in [153.3, 158.1, 175.5].into_iter().enumerate() {
                assert!((face2[k] - hand).abs() < 0.1, "{face2:?}");
            }
        },
    );
}

#[test]
fn a_face_tinted_and_highlighted_is_both_and_a_tint_of_none_or_white_is_exact() {
    // The front face tinted and highlighted: its inside tinted, its border outlined in the
    // selection's colour (green, picked unlike any tint); the tint takes nothing of the
    // outline's, which is drawn as without it. At 0 the tint is nothing: the plain frame,
    // whatever its colour (one below the body's in a channel too). At 1 the colour alone:
    // white, lit straight on, is 255 in every channel.
    on_lavapipe(
        "render::frame_tests::a_face_tinted_and_highlighted_is_both_and_a_tint_of_none_or_white_is_exact",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            const GREEN: [u8; 3] = [0, 255, 0];
            let face = [(1, Element::Face(3))];
            let lit_frame = Frame {
                outlines: Outlines {
                    selected: GREEN,
                    hovered: [255, 0, 255],
                },
                ..frame(&camera, &[1], &face, None)
            };
            let outlined = tinted_image(r, &lit_frame, &[], FEATURE_TINT);
            let both = tinted_image(r, &lit_frame, &face, FEATURE_TINT);
            let green = |p: &[u8; 4]| p[0] <= 2 && p[1] >= 253 && p[2] <= 2;
            let greens = |pixels: &[[u8; 4]]| -> Vec<usize> {
                (0..pixels.len()).filter(|&i| green(&pixels[i])).collect()
            };
            // Outlined about its border alone, tinted or not (the outline 1.5 points wide,
            // with its antialiasing: within 3.5 of the border).
            for image in [&outlined, &both] {
                let border = greens(image);
                assert!(border.len() > 40, "{} pixels outlined", border.len());
                for i in border {
                    let into = into_the_front(i);
                    assert!(into.abs() <= 3.5, "pixel {i}, {into} into the face");
                }
            }
            let centre = usize::try_from(26 * WIDTH + 32).unwrap();
            assert_colour(
                both[centre],
                tinted_by_hand(1.0, FEATURE_TINT.colour, 0.4),
                1.0,
                "the centre",
            );
            let plain_frame = frame(&camera, &[1], &[], None);
            let plain = tinted_image(r, &plain_frame, &[], FEATURE_TINT);
            for colour in [[0xd4, 0xdb, 0xff], [0, 0, 0], [255, 0, 255]] {
                let none = Tint {
                    colour,
                    amount: 0.0,
                };
                assert_eq!(
                    tinted_image(r, &plain_frame, &face, none),
                    plain,
                    "{colour:?}"
                );
            }
            let white = Tint {
                colour: [255; 3],
                amount: 1.0,
            };
            let whole = tinted_image(r, &plain_frame, &face, white);
            assert_colour(whole[centre], [255.0; 3], 1.0, "white whole");
            // The body's own colour at any amount: the plain frame.
            let body = Tint {
                colour: BODY,
                amount: 1.0,
            };
            assert_eq!(tinted_image(r, &plain_frame, &face, body), plain);
        },
    );
}

#[test]
fn a_hostile_tint_is_refused_naming_it_and_leaves_the_renderer_drawing() {
    on_lavapipe(
        "render::frame_tests::a_hostile_tint_is_refused_naming_it_and_leaves_the_renderer_drawing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let plain = frame(&camera, &[1], &[], None);
            let mut refused_by = |tinted: &[(u32, Element)], tint: Tint| {
                let asked = Frame {
                    tinted,
                    tint,
                    ..plain
                };
                let said = match r.prepare(&asked) {
                    Err(RenderFailure::Invalid(invalid)) => (invalid.parameter, invalid.message),
                    other => panic!("not refused: {:?}", other.err()),
                };
                // And the next frame drawn as ever.
                assert!(r.prepare(&plain).is_ok());
                said
            };
            let face = [(1, Element::Face(3))];
            // As highlights are: a body not drawn, a face its mesh hasn't.
            assert_eq!(
                refused_by(&[(2, Element::Face(1))], FEATURE_TINT),
                (
                    "tinted",
                    "tinted[0]'s body 2 isn't drawn in the frame".to_owned()
                )
            );
            assert_eq!(
                refused_by(
                    &[(1, Element::Face(1)), (1, Element::Face(7))],
                    FEATURE_TINT
                ),
                (
                    "tinted",
                    "tinted[1], Face(7) of body 1, isn't one of its 6 faces (numbered from 1)"
                        .to_owned()
                )
            );
            assert_eq!(
                refused_by(&[(1, Element::Face(0))], FEATURE_TINT).0,
                "tinted"
            );
            // Only faces are tinted: an edge or a vertex, each its body's, is refused.
            assert_eq!(
                refused_by(&[(1, Element::Edge(3))], FEATURE_TINT),
                (
                    "tinted",
                    "tinted[0], Edge(3) of body 1, isn't a face: only faces are tinted".to_owned()
                )
            );
            assert_eq!(
                refused_by(
                    &[(1, Element::Face(2)), (1, Element::Vertex(1))],
                    FEATURE_TINT
                ),
                (
                    "tinted",
                    "tinted[1], Vertex(1) of body 1, isn't a face: only faces are tinted"
                        .to_owned()
                )
            );
            // An amount not finite, or outside 0 to 1, with faces tinted or none.
            for amount in [
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                -0.01,
                1.01,
                -1.0,
            ] {
                let tint = Tint {
                    colour: [255; 3],
                    amount,
                };
                for tinted in [&face[..], &[]] {
                    assert_eq!(
                        refused_by(tinted, tint),
                        (
                            "tint",
                            format!("tint's amount {amount:?} must be finite, from 0 to 1")
                        )
                    );
                }
            }
            // A colour a channel below the body's (170, 175, 182) can't be added: refused.
            for colour in [[169, 255, 255], [255, 174, 255], [255, 255, 181], [0, 0, 0]] {
                let tint = Tint {
                    colour,
                    amount: 0.4,
                };
                assert_eq!(
                    refused_by(&face, tint),
                    (
                        "tint",
                        format!(
                            "tint's colour {colour:?} must be at least the body's, [170, 175, 182], in every channel: a tint is added to a face's colour, so it can make it lighter, never darker"
                        )
                    )
                );
            }
            // At the edges it's taken: the body's own colour, and the least amount above 0.
            for tint in [
                Tint {
                    colour: BODY,
                    amount: 0.4,
                },
                Tint {
                    colour: [255; 3],
                    amount: f32::MIN_POSITIVE,
                },
                Tint {
                    colour: [255; 3],
                    amount: -0.0,
                },
            ] {
                let asked = Frame {
                    tinted: &face,
                    tint,
                    ..plain
                };
                assert!(r.prepare(&asked).is_ok(), "{tint:?}");
            }
        },
    );
}

// ---- Vertices (M5.7): picked by a disc in the picking layer alone ----

/// The design's corner (M5.7 part 3, §4): a box 0 to 20 mm on each axis, orthographic about
/// (10, 10, 10) from along (1, 1, 1), z up, 80 mm over 800 × 800 pixels at a pixel a point
/// (10 pixels a mm). By hand, right = (−1, 1, 0)/√2 and up = (−1, −1, 2)/√6, so the corner
/// (20, 0, 20), its offset d = (10, −10, 10), lands at right·d = −20/√2 = −14.1421 mm and
/// up·d = 20/√6 = 8.1650 mm: pixel (400 − 141.421, 400 − 81.650) = (258.58, 318.35). It is
/// `Vertex5` in OCCT's box order. The top face (`Face6`, z = 20) lies between its edges along
/// −x and along +y, whose images leave it 30° above and below +x: a step along +x stays on
/// the top face at the corner's own depth (a = b along x and y keeps (1, 1, 1)·d), and a
/// pixel's centre half a row below it on the face that rises toward the eye, to (20, 20, 20):
/// the disc wins there by its reach toward the eye (0.012 × 80 = 0.96 mm), not by its depth
/// offset alone.
fn corner_camera(projection: Projection) -> Camera {
    Camera {
        projection,
        eye: [110.0, 110.0, 110.0],
        target: [10.0, 10.0, 10.0],
        up: [0.0, 0.0, 1.0],
    }
}

const CORNER_SIZE: [u32; 2] = [800, 800];

/// The pixel the corner (20, 0, 20) lands on: (400 − 200/√2, 400 − 200/√6).
fn corner_pixel() -> [f64; 2] {
    [400.0 - 200.0 / 2f64.sqrt(), 400.0 - 200.0 / 6f64.sqrt()]
}

/// A frame of the corner's box, body 1, picking the one pixel `at`, its vertices picked
/// within `vertices` points.
fn corner_frame<'a>(camera: &'a Camera, at: [i32; 2], vertices: Option<f32>) -> Frame<'a> {
    Frame {
        size_px: CORNER_SIZE,
        vertices,
        ..frame(camera, &[1], &[], Some(rect(at, [1, 1])))
    }
}

#[test]
fn a_corner_is_picked_within_its_disc_before_the_top_face_and_past_it_the_face_is() {
    on_lavapipe(
        "render::frame_tests::a_corner_is_picked_within_its_disc_before_the_top_face_and_past_it_the_face_is",
        |r| {
            r.upload(1, &Arc::new(box_mesh([0.0; 3], [20.0; 3])))
                .unwrap();
            let camera = corner_camera(Projection::Orthographic { view_height: 80.0 });
            let want = corner_pixel();
            assert!((want[0] - 258.5786).abs() < 1e-4 && (want[1] - 318.3503).abs() < 1e-4);
            assert_pixel(project(&camera, CORNER_SIZE, [20.0, 0.0, 20.0]), want, 1e-4);
            let corner = Element::Vertex(5);
            let top = Element::Face(6);
            // The pixel (258, 318), its centre 0.17 pixels from the corner's; 6 pixels along
            // +x, 5.92 from it, inside the 7-point disc (and 3 from each edge, past their
            // 1-point half width); 8 along, 7.92 from it, outside: the top face.
            for (at, want) in [
                ([258, 318], corner),
                ([264, 318], corner),
                ([266, 318], top),
            ] {
                assert_eq!(
                    hit(r, &corner_frame(&camera, at, Some(7.0))),
                    Some((1, want, at.map(|c| u32::try_from(c).unwrap()))),
                    "{at:?}"
                );
            }
            // The vertex's point is its own, exactly (its disc is drawn toward the eye).
            let picked = pick(r, &corner_frame(&camera, [264, 318], Some(7.0)));
            assert_eq!(picked.hit.unwrap().point, Some([20.0, 0.0, 20.0]));
            // With vertices filtered off the disc is left out, and hides nothing: 3 pixels
            // along +x is the top face (1.33 and 1.59 pixels from the two edges, past their
            // half width), and the corner's own pixel an edge or the face, never a vertex.
            assert_eq!(
                hit(r, &corner_frame(&camera, [261, 318], None)),
                Some((1, top, [261, 318]))
            );
            assert_eq!(
                hit(r, &corner_frame(&camera, [261, 318], Some(7.0))),
                Some((1, corner, [261, 318]))
            );
            let bare = hit(r, &corner_frame(&camera, [258, 318], None));
            assert!(
                matches!(bare, Some((1, Element::Edge(_) | Element::Face(_), _))),
                "{bare:?}"
            );
            // A smaller disc is a smaller tolerance: 4 points leaves the pixel 5.92 away to
            // the face, and two pixels a point doubles it in pixels, reaching it again.
            assert_eq!(
                hit(r, &corner_frame(&camera, [264, 318], Some(4.0))),
                Some((1, top, [264, 318]))
            );
            assert_eq!(
                hit(
                    r,
                    &Frame {
                        pixels_per_point: 2.0,
                        ..corner_frame(&camera, [264, 318], Some(4.0))
                    }
                ),
                Some((1, corner, [264, 318]))
            );
            // Its reach is the radius in points times the pixels a point, 5 × 2 = 10 pixels,
            // the discs near the picking taken within that and a pixel more; not 5 + 2 = 7,
            // within 8: the pixel 9 along, 8.92 from the corner, is the corner's.
            assert_eq!(
                hit(
                    r,
                    &Frame {
                        pixels_per_point: 2.0,
                        ..corner_frame(&camera, [267, 318], Some(5.0))
                    }
                ),
                Some((1, corner, [267, 318]))
            );
        },
    );
}

/// The corner nearest the eye, (20, 20, 20), at the corner camera's offset from its target
/// d = (10, 10, 10): right·d = 0 and up·d = (−10 − 10 + 20)/√6 = 0, so it lands at the image's
/// centre, (400, 400), and the pixel (400, 400) has its centre 0.71 pixels from it. It is the
/// scene's front: the orthographic camera stands its margin (1 mm, past 1% of the reach
/// 10√3 = 17.32 mm) in front of it. At 100 mm high the disc's reach, 0.012 × 100 = 1.2 mm,
/// is past that margin, and a disc moved all of it would be cut by the near plane and leave
/// the corner to its edges and faces; at 80 mm, 0.96 mm, it is not.
#[test]
fn the_corner_nearest_an_orthographic_eye_is_picked_however_high_the_view() {
    on_lavapipe(
        "render::frame_tests::the_corner_nearest_an_orthographic_eye_is_picked_however_high_the_view",
        |r| {
            r.upload(1, &Arc::new(box_mesh([0.0; 3], [20.0; 3])))
                .unwrap();
            // (x1, y1, z1): `Vertex7` in the box's order ([`box_mesh`]).
            let front = Element::Vertex(7);
            for view_height in [80.0, 100.0, 400.0, 4000.0] {
                let camera = corner_camera(Projection::Orthographic { view_height });
                assert_pixel(
                    project(&camera, CORNER_SIZE, [20.0, 20.0, 20.0]),
                    [400.0, 400.0],
                    1e-9,
                );
                let picked = pick(r, &corner_frame(&camera, [400, 400], Some(7.0)));
                let hit = picked.hit.unwrap();
                assert_eq!(
                    (hit.body, hit.hit),
                    (1, front),
                    "a view {view_height} mm high"
                );
                assert_eq!(hit.point, Some([20.0, 20.0, 20.0]));
            }
        },
    );
}

#[test]
fn a_vertex_nearer_a_perspective_eye_than_it_draws_is_never_picked() {
    on_lavapipe(
        "render::frame_tests::a_vertex_nearer_a_perspective_eye_than_it_draws_is_never_picked",
        |r| {
            r.upload(1, &Arc::new(box_mesh([0.0; 3], [20.0; 3])))
                .unwrap();
            // The eye along (1, −1, 1) from the corner (20, 0, 20), looking at it: the corner
            // at the image's centre. 0.75 nm off, the corner is nearer than the 1 nm the
            // drawing cuts at (and past the near plane, half that, so a disc there would be
            // drawn): none projects and none is picked. 2 nm off, it's drawn and picked.
            let u = [1.0, -1.0, 1.0].map(|c: f64| c / 3f64.sqrt());
            let looking = |off: f64| Camera {
                projection: Projection::Perspective {
                    field_of_view: 60.0,
                },
                eye: [20.0 + u[0] * off, u[1] * off, 20.0 + u[2] * off],
                target: [20.0 - u[0], -u[1], 20.0 - u[2]],
                up: [0.0, 0.0, 1.0],
            };
            let near = looking(0.75 * MIN_EYE_DISTANCE);
            assert_eq!(project(&near, CORNER_SIZE, [20.0, 0.0, 20.0]), None);
            // The faces about the corner cut away there, the box is seen through them from
            // inside: the centre's line of sight, (20, 0, 20) − t(1, −1, 1)/√3, meets the far
            // corner (0, 20, 0), Vertex4, at t = 20√3, which is picked; never Vertex5.
            // Without the discs, the far corner's edge along +x, Edge5 (0, 20, 0)–(20, 20, 0):
            // seen along the diagonal its image leaves the corner 30° below the row, so the
            // pixel's centre, (0.5, 0.5) from the corner's image, is 0.18 pixels off its line,
            // within its stroke. Seen from inside, the corner is concave: the floor beside the
            // edge there is nearer the eye than its line by √1.5 × 0.18 pixels' width (0.05
            // mm at 34.6 mm), 0.011 mm, inside the edge's reach, 2/800 of 2 tan 30° of its
            // depth (0.1 mm): its picking strip wins the pixel (N5; before it, a face did).
            let without = hit(r, &corner_frame(&near, [400, 400], None));
            assert_eq!(without, Some((1, Element::Edge(5), [400, 400])));
            assert_eq!(
                hit(r, &corner_frame(&near, [400, 400], Some(7.0))),
                Some((1, Element::Vertex(4), [400, 400]))
            );
            let far = looking(2.0 * MIN_EYE_DISTANCE);
            assert_pixel(
                project(&far, CORNER_SIZE, [20.0, 0.0, 20.0]),
                [400.0, 400.0],
                1e-6,
            );
            assert_eq!(
                hit(r, &corner_frame(&far, [400, 400], Some(7.0))),
                Some((1, Element::Vertex(5), [400, 400]))
            );
        },
    );
}

#[test]
fn a_vertex_disc_draws_nothing_of_the_image() {
    on_lavapipe(
        "render::frame_tests::a_vertex_disc_draws_nothing_of_the_image",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // From the front, the corner (0, 0, 10) lands at (9.09, 20.73): the picking
            // there with its vertices' discs, and without, draw the same pixels, and the
            // discs pick it.
            let at_corner = Some(rect([9, 20], [1, 1]));
            let with = Frame {
                vertices: Some(7.0),
                ..frame(&camera, &[1], &[], at_corner)
            };
            let commands = r.prepare(&with).unwrap();
            let discs = composited(r, Some(commands));
            let commands = r
                .prepare(&Frame {
                    vertices: None,
                    ..with
                })
                .unwrap();
            assert_eq!(composited(r, Some(commands)), discs);
            assert_eq!(hit(r, &with), Some((1, Element::Vertex(1), [9, 20])));
        },
    );
}

#[test]
fn a_frames_vertices_and_its_vertex_highlights_are_checked() {
    on_lavapipe(
        "render::frame_tests::a_frames_vertices_and_its_vertex_highlights_are_checked",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let good = frame(&camera, &[1], &[], Some(ON_THE_FACE));
            for radius in [0.0, -0.0, -7.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let hostile = Frame {
                    vertices: Some(radius),
                    ..good
                };
                assert_eq!(refused(r, &hostile), "vertices", "{radius}");
            }
            // Any finite radius above 0 is drawn: a subnormal's disc covers nothing.
            for radius in [f32::MIN_POSITIVE / 2.0, 7.0, 1e6] {
                r.prepare(&Frame {
                    vertices: Some(radius),
                    ..good
                })
                .unwrap();
            }
            // A vertex highlighted or hovered must be the body's, as a face is: drawn by the
            // caller, never outlined here, so it changes no pixel and no picking ID.
            for vertex in [0, 9, u32::MAX] {
                let lit = [(1, Element::Vertex(vertex))];
                assert_eq!(
                    refused(r, &frame(&camera, &[1], &lit, None)),
                    "highlights",
                    "{vertex}"
                );
                assert_eq!(
                    refused(
                        r,
                        &Frame {
                            hovered: &lit,
                            ..good
                        }
                    ),
                    "hovered",
                    "{vertex}"
                );
            }
            let lit = [(1, Element::Vertex(8))];
            let commands = r.prepare(&frame(&camera, &[1], &lit, None)).unwrap();
            let marked = composited(r, Some(commands));
            let commands = r.prepare(&frame(&camera, &[1], &[], None)).unwrap();
            assert_eq!(composited(r, Some(commands)), marked);
            assert_eq!(
                hit(
                    r,
                    &Frame {
                        hovered: &lit,
                        ..frame(&camera, &[1], &lit, Some(ON_THE_FACE))
                    }
                ),
                Some((1, Element::Face(3), [32, 26]))
            );
        },
    );
}

#[test]
fn a_vertex_behind_a_face_is_picked_within_the_reach_toward_the_eye_and_not_past_it() {
    on_lavapipe(
        "render::frame_tests::a_vertex_behind_a_face_is_picked_within_the_reach_toward_the_eye_and_not_past_it",
        |r| {
            // The front view's box, its vertices put behind its front face (y = 0) at
            // (10, y, 5) and (30, y, 5): at columns 32 + (10 − 21) × 12/11 = 20 and
            // 32 + 9 × 12/11 = 41.82, row 24 + 2 × 12/11 = 26.18. The view is 44 mm high,
            // so the reach toward the eye is 0.012 × 44 = 0.528 mm: 0.5 behind is picked,
            // 0.56 behind isn't (the face is), nor is the back face's middle, 20 behind.
            let mut mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            mesh.vertex_points = vec![[10.0, 0.5, 5.0], [30.0, 0.56, 5.0], [20.0, 20.0, 5.0]];
            r.upload(1, &Arc::new(mesh)).unwrap();
            let camera = front();
            let at = |x: i32| Frame {
                vertices: Some(7.0),
                ..frame(&camera, &[1], &[], Some(rect([x, 26], [1, 1])))
            };
            assert_eq!(hit(r, &at(20)), Some((1, Element::Vertex(1), [20, 26])));
            assert_eq!(hit(r, &at(41)), Some((1, Element::Face(3), [41, 26])));
            assert_eq!(hit(r, &at(32)), Some((1, Element::Face(3), [32, 26])));
        },
    );
}

#[test]
fn a_vertex_behind_a_face_in_perspective_is_picked_within_its_reach_along_its_line_and_not_past_it()
{
    on_lavapipe(
        "render::frame_tests::a_vertex_behind_a_face_in_perspective_is_picked_within_its_reach_along_its_line_and_not_past_it",
        |r| {
            // The front face (y = 0) seen square on from (20, −100, 5), 60° high: a vertex at
            // (20, y, 5) lands on the image's centre, (32, 24), 100 + y along the view. Its
            // disc goes toward the eye by 0.012 × 2 tan 30° = 0.013856 of that, so it lies
            // before the face, 100 along, while (100 + y)(1 − 0.013856) < 100: y < 1.405 mm.
            // 1.2 behind is picked; 1.6 behind, the face is.
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 60.0,
                },
                eye: [20.0, -100.0, 5.0],
                target: [20.0, 0.0, 5.0],
                up: [0.0, 0.0, 1.0],
            };
            assert_pixel(
                project(&camera, [WIDTH, HEIGHT], [20.0, 1.2, 5.0]),
                [32.0, 24.0],
                1e-9,
            );
            for (behind, want) in [(1.2, Element::Vertex(1)), (1.6, Element::Face(3))] {
                let mut mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
                mesh.vertex_points = vec![[20.0, behind, 5.0]];
                r.upload(1, &Arc::new(mesh)).unwrap();
                let asking = Frame {
                    vertices: Some(7.0),
                    ..frame(&camera, &[1], &[], Some(rect([32, 24], [1, 1])))
                };
                assert_eq!(
                    hit(r, &asking),
                    Some((1, want, [32, 24])),
                    "{behind} behind"
                );
            }
        },
    );
}

// ---- Hostile vertices ----

#[test]
fn a_crowd_of_vertices_on_one_pixel_picks_one_of_them_and_the_frame_still_draws() {
    on_lavapipe(
        "render::frame_tests::a_crowd_of_vertices_on_one_pixel_picks_one_of_them_and_the_frame_still_draws",
        |r| {
            // 200,000 vertices on the front face's centre, (21, 0, 7), and one more off the
            // frame: the picking draws a disc for each near its square, and picks a vertex
            // there (whichever the GPU drew last of those at one depth); the frames after,
            // with no picking, draw none.
            let mut mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            mesh.vertex_points = vec![[21.0, 0.0, 7.0]; 200_000];
            mesh.vertex_points.push([1e9, 0.0, 7.0]);
            r.upload(1, &Arc::new(mesh)).unwrap();
            let camera = front();
            let asking = Frame {
                vertices: Some(7.0),
                ..frame(&camera, &[1], &[], Some(rect([32, 24], [1, 1])))
            };
            let found = hit(r, &asking);
            assert!(
                matches!(found, Some((1, Element::Vertex(1..=200_000), [32, 24]))),
                "{found:?}"
            );
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))),
                Some((1, Element::Face(3), [32, 26]))
            );
        },
    );
}

#[test]
fn a_perspective_as_wide_as_170_degrees_draws_a_disc_toward_the_eye_a_quarter_of_its_depth_at_most()
{
    on_lavapipe(
        "render::frame_tests::a_perspective_as_wide_as_170_degrees_draws_a_disc_toward_the_eye_a_quarter_of_its_depth_at_most",
        |r| {
            // From (20, −10, 5), looking along +y at the front face's middle (y = 0, 10 mm
            // off), 170° wide: the view's height at a depth d is 2 tan 85° d = 22.86 d, so
            // 1.2% of it is 0.274 d, past the most, a quarter. A vertex 3.5 mm behind the
            // face, 13.5 mm off, has its disc 10.125 mm off, behind the face (0.274 d would
            // put it at 9.80, ahead of it): the face is picked. One 2.6 behind, 12.6 off, has
            // its disc at 9.45: picked. The box is 100 mm tall about the eye's height (z from
            // −45 to 55): at 4.76 mm a pixel there, its top and bottom edges are 10.5 rows
            // from the centre, and its sides 4.2 columns, so no edge's stroke reaches the
            // pixel (32, 24), whose picking strip, drawn a quarter of its depth toward the eye
            // as the disc is (N5), would take it: the bench's box, 10 mm tall, is 2.1 rows
            // tall here, every pixel of it within 0.55 of its top or bottom edge's line.
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 170.0,
                },
                eye: [20.0, -10.0, 5.0],
                target: [20.0, 0.0, 5.0],
                up: [0.0, 0.0, 1.0],
            };
            let centre = Some(rect([32, 24], [1, 1]));
            for (behind, want) in [(3.5, Element::Face(3)), (2.6, Element::Vertex(1))] {
                let mut mesh = box_mesh([0.0, 0.0, -45.0], [40.0, 20.0, 100.0]);
                mesh.vertex_points = vec![[20.0, behind, 5.0]];
                r.upload(1, &Arc::new(mesh)).unwrap();
                let asking = Frame {
                    vertices: Some(7.0),
                    ..frame(&camera, &[1], &[], centre)
                };
                assert_eq!(hit(r, &asking), Some((1, want, [32, 24])), "{behind}");
            }
        },
    );
}

/// What the app's selection takes of a picking (`Selection::under`, M5.7): a vertex at its
/// centre pixel, else an edge in its square, else a face at its centre pixel.
fn taken(picking: &Picking, centre: [u32; 2]) -> Option<(u32, Element)> {
    let at_centre = |vertex: bool| {
        picking
            .hit
            .filter(|pick| pick.pixel == centre && matches!(pick.hit, Element::Vertex(_)) == vertex)
    };
    let face = at_centre(false).filter(|pick| matches!(pick.hit, Element::Face(_)));
    at_centre(true)
        .or(picking.edge)
        .or(face)
        .map(|pick| (pick.body, pick.hit))
}

#[test]
fn the_casts_first_is_what_the_renderer_picks_but_where_the_square_and_the_stroke_reach_past_it() {
    on_lavapipe(
        "render::frame_tests::the_casts_first_is_what_the_renderer_picks_but_where_the_square_and_the_stroke_reach_past_it",
        |r| {
            // The 20 mm cube in the bench's numbering (`box_mesh`: Face6 its top, Face3 its
            // front, Edge7 along x at y = z = 20, Edge11 along y at x = z = 20, Vertex7 the
            // corner (20, 20, 20)), drawn and cast on alike. Each pixel's square is the
            // selection's, the prototype's 6 points about it (`HIT_EDGE_POINTS`, the cast's
            // edge threshold too), and its vertices are picked at the cast's 7 points; the
            // cast is at the pixel's centre.
            let mesh = Arc::new(box_mesh([0.0; 3], [20.0; 3]));
            r.upload(1, &mesh).unwrap();
            let pickable = Pickable::new(Arc::clone(&mesh));
            let iso = corner_camera(Projection::Orthographic { view_height: 80.0 });
            let wide = corner_camera(Projection::Perspective {
                field_of_view: 40.0,
            });
            // Orthographic from above the front at 45 degrees, 40 mm over 400 rows: the top's
            // back edge (Edge7) at row 200 − 200/√2 = 58.58.
            let oblique = Camera {
                projection: Projection::Orthographic { view_height: 40.0 },
                eye: [10.0, -90.0, 110.0],
                target: [10.0, 10.0, 10.0],
                up: [0.0, 0.0, 1.0],
            };
            let square = [400, 400];
            let on = |camera: &Camera, size: [u32; 2], p: [f64; 3]| -> [i32; 2] {
                project(camera, size, p).unwrap().map(|c| c.floor() as i32)
            };
            use Element::{Edge, Face, Vertex};
            let cases = [
                // The corner nearest the eye, every edge from it going away: the vertex.
                (&iso, CORNER_SIZE, [400, 400], Some(Vertex(7))),
                (&wide, CORNER_SIZE, [400, 400], Some(Vertex(7))),
                // The top's middle, 10 mm from any edge.
                (
                    &iso,
                    CORNER_SIZE,
                    on(&iso, CORNER_SIZE, [10.0, 10.0, 20.0]),
                    Some(Face(6)),
                ),
                (
                    &wide,
                    CORNER_SIZE,
                    on(&wide, CORNER_SIZE, [10.0, 10.0, 20.0]),
                    Some(Face(6)),
                ),
                // A tenth of a millimetre in from the top's edge at x = 20, under a pixel: the
                // edge, in the band, and first within its reach.
                (
                    &iso,
                    CORNER_SIZE,
                    on(&iso, CORNER_SIZE, [19.9, 10.0, 20.0]),
                    Some(Edge(11)),
                ),
                // Off the cube: nothing.
                (&iso, CORNER_SIZE, [40, 40], None),
                // A pixel below the top's back edge, which the top recedes to: the edge. (At
                // the pixel 60, 1.92 below it, the top is 0.192 mm nearer the eye than the
                // edge against its reach of 0.2: first still, but by 0.008 mm.)
                (&oblique, square, [200, 59], Some(Edge(7))),
                (
                    &oblique,
                    square,
                    on(&oblique, square, [10.0, 10.0, 20.0]),
                    Some(Face(6)),
                ),
                (
                    &oblique,
                    square,
                    on(&oblique, square, [10.0, 0.0, 10.0]),
                    Some(Face(3)),
                ),
            ];
            // What the renderer picks at `at` as the selection takes it, and the cast's first.
            let mut both = |camera: &Camera, size: [u32; 2], at: [i32; 2]| {
                let band = HIT_EDGE_POINTS as i32;
                let side = 2 * band.unsigned_abs() + 1;
                let picking = rect([at[0] - band, at[1] - band], [side, side]);
                let asking = Frame {
                    size_px: size,
                    vertices: Some(HIT_VERTEX_POINTS as f32),
                    ..frame(camera, &[1], &[], Some(picking))
                };
                let centre = at.map(|c| u32::try_from(c).unwrap());
                let gpu = taken(&pick(r, &asking), centre);
                let pointer = at.map(|c| f64::from(c) + 0.5);
                let cast = hits(camera, size, 1.0, &[(1, &pickable)], pointer);
                assert!(cast.first().is_none_or(|hit| !hit.hidden), "{cast:?}");
                (gpu, cast.first().map(|hit| (hit.body, hit.element)))
            };
            for (camera, size, at, want) in cases {
                let (gpu, cast) = both(camera, size, at);
                assert_eq!(gpu, want.map(|e| (1, e)), "the renderer at {at:?}");
                assert_eq!(cast, gpu, "the cast at {at:?}");
            }
            // Where they part: the hover takes an edge drawn anywhere in its square, and the
            // edge is drawn 2 points wide about its line, so it reaches an edge whose line is
            // up to 6 points out along a row or a column, and a point more for the stroke's
            // half; the cast takes one nearer the pointer than 6 points, measured to its line.
            // From above (40 mm over 400 rows, 10 pixels a millimetre), the top's edge at
            // x = 20 is column 300, and the pixel 293's centre is 6.5 pixels off it: the
            // square reaches pixel 299, whose centre is half a pixel off the line, drawn
            // over, so the renderer gives the edge; the cast, past its 6, the top.
            let above = Camera {
                projection: Projection::Orthographic { view_height: 40.0 },
                eye: [10.0, 10.0, 100.0],
                target: [10.0, 10.0, 10.0],
                up: [0.0, 1.0, 0.0],
            };
            assert_eq!(
                project(&above, square, [20.0, 15.0, 20.0]).map(|p| p[0].round()),
                Some(300.0)
            );
            assert_eq!(
                both(&above, square, [293, 150]),
                (Some((1, Edge(11))), Some((1, Face(6))))
            );
            // 5.5 pixels off, both the edge; 7.5 off, the square's last pixel is 1.5 off the
            // line, not drawn over: both the top.
            assert_eq!(
                both(&above, square, [294, 150]),
                (Some((1, Edge(11))), Some((1, Edge(11))))
            );
            assert_eq!(
                both(&above, square, [292, 150]),
                (Some((1, Face(6))), Some((1, Face(6))))
            );
            // Where the face beside the edge rises toward the eye, the edge's picking strip
            // wins its stroke there only within its reach toward the eye, 2 pixels of depth:
            // 0.2 mm (2/400 of 40). The oblique view's back edge is at row 58.58, and at row
            // 59's centre, 0.92 pixels below it (0.092 mm down the image), the top is 0.092
            // mm nearer the eye than the edge's line (its depth falls a millimetre a
            // millimetre down the image): within the reach, so the stroke is the edge's
            // there, and row 60's centre, 1.92 below, is past its half width. So the renderer
            // gives the edge to a square reaching row 59: from the pixel 65 up (6.92 off it),
            // not from 66 (7.92 off), where both give the top. The cast lists the edge within
            // 6 pixels, but first only within the same reach: at 61 (2.92 off) the top is
            // 0.292 nearer, past it, so the top comes first and the edge after it, while the
            // hover takes the edge; so to 64 (5.92 off), and at 65 (6.92 off) the cast lists
            // no edge.
            assert_eq!(
                both(&oblique, square, [200, 61]),
                (Some((1, Edge(7))), Some((1, Face(6))))
            );
            assert_eq!(
                both(&oblique, square, [200, 64]),
                (Some((1, Edge(7))), Some((1, Face(6))))
            );
            assert_eq!(
                both(&oblique, square, [200, 65]),
                (Some((1, Edge(7))), Some((1, Face(6))))
            );
            assert_eq!(
                both(&oblique, square, [200, 66]),
                (Some((1, Face(6))), Some((1, Face(6))))
            );
        },
    );
}

// ---- Concave edges (N5) ----
//
// The design study's scenes (lane C, n5-concave-edges.md), each value worked by hand there and
// again here. An edge's stroke is drawn square to the view at its line's depth, so at a
// concave edge, whose faces both rise toward the eye away from its line, they hid every pixel
// of it: a frame that picks draws each edge near its square again, into the picking layer
// alone, toward the eye by 2 pixels of the view's depth (`edge_reach`, never more than a
// vertex's 1.2% of the view's height). Each square is the selection's, 6 points out about
// its pixel (13 by 13), at a pixel a point, with no vertices picked.

/// An L extruded along y from 0 to 20, its profile in x and z (0, 0), (20, 0), (20, 5),
/// (5, 5), (5, 20), (0, 20), numbered by hand: Face1 the floor's top (z = 5, x 5 to 20),
/// Face2 the wall's inner side (x = 5, z 5 to 20), Face3 the upright's top (z = 20, x 0 to 5),
/// Face4 the back (x = 0), Face5 the bottom (z = 0), Face6 the end (x = 20, z 0 to 5), Face7
/// and Face8 its sides (y = 0 and y = 20), two quads each. Edge1 the concave edge
/// (5, 0, 5)–(5, 20, 5), where the floor meets the wall; Edge2 (0, 0, 0)–(0, 0, 20); Edge3 to
/// Edge7 along y at the profile's other corners, (20, 5), (5, 20) (the wall's top, convex),
/// (0, 20), (0, 0), and (20, 0); then the profile's other edges at y = 0 and all six at y = 20.
/// Its vertices the profile's corners at y = 0, then at y = 20.
fn l_mesh() -> Mesh {
    /// A quad's corners, in order round it.
    type Corners = [[f64; 3]; 4];
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
    let at = |y: f64, quad: [[f64; 2]; 4]| quad.map(|[x, z]| [x, y, z]);
    let side = |y: f64| {
        [
            at(y, [[0.0, 0.0], [5.0, 0.0], [5.0, 20.0], [0.0, 20.0]]),
            at(y, [[5.0, 0.0], [20.0, 0.0], [20.0, 5.0], [5.0, 5.0]]),
        ]
    };
    let along =
        |[x, z]: [f64; 2], [u, w]: [f64; 2]| [[x, 0.0, z], [u, 0.0, w], [u, 20.0, w], [x, 20.0, z]];
    let faces: [(Vec<Corners>, [f32; 3]); 8] = [
        (vec![along([5.0, 5.0], [20.0, 5.0])], [0.0, 0.0, 1.0]),
        (vec![along([5.0, 5.0], [5.0, 20.0])], [1.0, 0.0, 0.0]),
        (vec![along([0.0, 20.0], [5.0, 20.0])], [0.0, 0.0, 1.0]),
        (vec![along([0.0, 0.0], [0.0, 20.0])], [-1.0, 0.0, 0.0]),
        (vec![along([0.0, 0.0], [20.0, 0.0])], [0.0, 0.0, -1.0]),
        (vec![along([20.0, 0.0], [20.0, 5.0])], [1.0, 0.0, 0.0]),
        (side(0.0).to_vec(), [0.0, -1.0, 0.0]),
        (side(20.0).to_vec(), [0.0, 1.0, 0.0]),
    ];
    for (quads, normal) in faces {
        for quad in &quads {
            let base = u32::try_from(mesh.positions.len()).unwrap();
            mesh.positions.extend(quad);
            mesh.normals.extend([normal; 4]);
            mesh.triangles
                .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        }
        mesh.face_triangles
            .push(u32::try_from(2 * quads.len()).unwrap());
    }
    let profile: [[f64; 2]; 6] = [
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 5.0],
        [5.0, 5.0],
        [5.0, 20.0],
        [0.0, 20.0],
    ];
    let mut edges: Vec<([f64; 3], [f64; 3])> = vec![
        ([5.0, 0.0, 5.0], [5.0, 20.0, 5.0]),
        ([0.0, 0.0, 0.0], [0.0, 0.0, 20.0]),
    ];
    for [x, z] in [
        [20.0, 5.0],
        [5.0, 20.0],
        [0.0, 20.0],
        [0.0, 0.0],
        [20.0, 0.0],
    ] {
        edges.push(([x, 0.0, z], [x, 20.0, z]));
    }
    for y in [0.0, 20.0] {
        for (k, &[x, z]) in profile.iter().enumerate() {
            let [u, w] = profile[(k + 1) % 6];
            // Edge2 is the profile's last at y = 0, (0, 0, 20) to (0, 0, 0), given first.
            if y == 0.0 && k == 5 {
                continue;
            }
            edges.push(([x, y, z], [u, y, w]));
        }
    }
    for (a, b) in edges {
        mesh.edge_points.extend([a, b]);
        mesh.edge_point_counts.push(2);
    }
    for y in [0.0, 20.0] {
        mesh.vertex_points
            .extend(profile.iter().map(|&[x, z]| [x, y, z]));
    }
    mesh
}

/// The L seen from (105, 110, 105) about the concave edge's middle, (5, 10, 5), z up,
/// orthographic and 100 mm high over 800 rows: 0.125 mm a pixel. Its axes by hand: right
/// (−1, 1, 0)/√2, up (−1, −1, 2)/√6, back (1, 1, 1)/√3; a point p lands at column 400 +
/// 8 right · (p − (5, 10, 5)) and row 400 − 8 up · (p − (5, 10, 5)). Edge1, (5, y, 5), lands
/// at (400 + 5.6569 (y − 10), 400 + 3.2660 (y − 10)): through the image's centre, 30° below
/// the row, its pixel's centre (400.5, 400.5) 0.18 pixels off it. Each face beside it rises
/// toward the eye away from its line by √1.5 mm a millimetre across its image, 0.153 mm a
/// pixel, so its drawn stroke loses to them 2e-5 pixels off it; its reach, 2 pixels of
/// depth, is 0.25 mm. The camera stands 18.71 + 1 mm in front of the target (the L's
/// farthest point, (20, 20, 0), √350 away), so no cap is near.
fn l_camera() -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 100.0 },
        eye: [105.0, 110.0, 105.0],
        target: [5.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

const SQUARE: [u32; 2] = [800, 800];

/// A frame of `bodies`, 800 by 800 at a pixel a point, picking the selection's square about
/// the pixel `at` (6 points out: 13 by 13), its vertices not picked.
fn square_frame<'a>(camera: &'a Camera, bodies: &'a [u32], at: [u32; 2]) -> Frame<'a> {
    band_frame(camera, bodies, at, SQUARE)
}

/// [`square_frame`], `size` pixels.
fn band_frame<'a>(
    camera: &'a Camera,
    bodies: &'a [u32],
    at: [u32; 2],
    size: [u32; 2],
) -> Frame<'a> {
    let band = HIT_EDGE_POINTS as i32;
    let side = 2 * band.unsigned_abs() + 1;
    let at = at.map(|c| i32::try_from(c).unwrap());
    Frame {
        size_px: size,
        ..frame(
            camera,
            bodies,
            &[],
            Some(rect([at[0] - band, at[1] - band], [side, side])),
        )
    }
}

/// The distance from `p` to the line through `a` along `direction` (of unit length).
fn from_line(p: [f64; 3], a: [f64; 3], direction: [f64; 3]) -> f64 {
    let d = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let along = d[0] * direction[0] + d[1] * direction[1] + d[2] * direction[2];
    norm([
        d[0] - along * direction[0],
        d[1] - along * direction[1],
        d[2] - along * direction[2],
    ])
}

#[test]
fn a_concave_edge_is_picked_along_its_stroke_and_past_its_band_the_faces_beside_it_are() {
    on_lavapipe(
        "render::frame_tests::a_concave_edge_is_picked_along_its_stroke_and_past_its_band_the_faces_beside_it_are",
        |r| {
            use Element::{Edge, Face};
            let mesh = Arc::new(l_mesh());
            r.upload(1, &mesh).unwrap();
            let camera = l_camera();
            assert_pixel(
                project(&camera, SQUARE, [5.0, 10.0, 5.0]),
                [400.0, 400.0],
                1e-4,
            );
            assert_pixel(
                project(&camera, SQUARE, [5.0, 20.0, 5.0]),
                [400.0 + 80.0 / 2f64.sqrt(), 400.0 + 80.0 / 6f64.sqrt()],
                1e-4,
            );
            // Each pixel's centre's signed distance from Edge1's image, the floor's side
            // positive: s = 0.866 (row − 400) − 0.5 (column − 400), at the centre. Within
            // its stroke (1 point either side), the face beside it is at most 0.153 mm
            // nearer the eye than its line, and its picking strip 0.25 mm: the edge's. Its
            // square reaches the stroke from 1 or 2 pixels off either side, and within the
            // band's 6 the selection takes the edge over the face; 10 pixels off, the
            // square misses the stroke (it comes within 2.38 pixels of the line on the
            // floor's side, 2.01 on the wall's), and the face under the centre is taken.
            for (at, s, want) in [
                ([400, 400], 0.18, Edge(1)),
                ([400, 401], 1.05, Edge(1)),
                ([400, 402], 1.92, Edge(1)),
                ([400, 399], -0.68, Edge(1)),
                ([400, 398], -1.55, Edge(1)),
                ([400, 412], 10.58, Face(1)),
                ([400, 388], -10.21, Face(2)),
            ] {
                let centre = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 0.5];
                let off = 0.75f64.sqrt() * (centre[1] - 400.0) - 0.5 * (centre[0] - 400.0);
                assert!((off - s).abs() < 0.01, "{at:?}: {off}");
                let picking = pick(r, &square_frame(&camera, &[1], at));
                assert_eq!(taken(&picking, at), Some((1, want)), "{at:?}");
            }
            // The edge's own pixel is the edge's in the ID buffer itself, its point the
            // drawn line's there, its reach toward the eye undone. The pixel's centre is
            // (0.5, 0.5) from the line's point (5, 10, 5): along the line's image, (0.866,
            // 0.5), 0.683 pixels, of the 8 √(2/3) = 6.532 it moves a millimetre of y; across
            // it 0.183 pixels (0.0229 mm), square to the view and the line, along
            // (1, 0, −1)/√2, the floor's side. Without the undoing it would be 0.25 mm along
            // the view, 0.20 mm off the line.
            let picking = pick(r, &square_frame(&camera, &[1], [400, 400]));
            let hit = picking.hit.unwrap();
            assert_eq!((hit.body, hit.hit, hit.pixel), (1, Edge(1), [400, 400]));
            let half = 0.75f64.sqrt() / 2.0;
            let along = (half + 0.25) / (8.0 * (2.0f64 / 3.0).sqrt());
            let across = (half - 0.25) * 0.125 / 2f64.sqrt();
            let want = [5.0 + across, 10.0 + along, 5.0 - across];
            assert!((along - 0.104_564).abs() < 1e-6 && (across - 0.016_176).abs() < 1e-6);
            assert_point(&hit, want, [2e-3; 3]);
            let point = hit.point.unwrap();
            let off = from_line(point, [5.0, 0.0, 5.0], [0.0, 1.0, 0.0]);
            assert!(
                (off - 0.022_876).abs() < 2e-3,
                "{point:?} is {off} off Edge1"
            );
        },
    );
}

#[test]
fn an_edge_behind_the_body_stays_unpickable_and_the_cast_marks_it_hidden() {
    on_lavapipe(
        "render::frame_tests::an_edge_behind_the_body_stays_unpickable_and_the_cast_marks_it_hidden",
        |r| {
            use Element::{Edge, Face};
            // Edge2, the back's corner along z at (0, 0), behind the wall from this side: at
            // z = 10 it lands at (400 − 40/√2, 400 − 200/√6) = (371.716, 318.35), and the pixel
            // (371, 318) has its centre 0.22 pixels off its line. The ray through it toward
            // the eye meets the wall, Face2, at (5, 5, 15), 5√3 = 8.66 mm nearer: far past its
            // reach. The nearest edge drawn in view, the wall's front at (5, 0, z), is down
            // column 343.4, 28 pixels away: the square holds no edge.
            let mesh = Arc::new(l_mesh());
            r.upload(1, &mesh).unwrap();
            let camera = l_camera();
            assert_pixel(
                project(&camera, SQUARE, [0.0, 0.0, 10.0]),
                [400.0 - 40.0 / 2f64.sqrt(), 400.0 - 200.0 / 6f64.sqrt()],
                1e-4,
            );
            let at = [371, 318];
            let picking = pick(r, &square_frame(&camera, &[1], at));
            assert_eq!(picking.edge, None);
            let hit = picking.hit.unwrap();
            assert_eq!((hit.body, hit.hit, hit.pixel), (1, Face(2), at));
            let pickable = Pickable::new(Arc::clone(&mesh));
            let cast = hits(&camera, SQUARE, 1.0, &[(1, &pickable)], [371.5, 318.5]);
            let first = cast.first().unwrap();
            assert_eq!((first.element, first.hidden), (Face(2), false), "{cast:?}");
            let edge = cast.iter().find(|h| h.element == Edge(2)).unwrap();
            assert!(edge.hidden, "{cast:?}");
        },
    );
}

#[test]
fn a_convex_edge_is_picked_along_its_stroke_as_before() {
    on_lavapipe(
        "render::frame_tests::a_convex_edge_is_picked_along_its_stroke_as_before",
        |r| {
            use Element::{Edge, Face};
            // Edge4, the wall's top (5, y, 20), where it meets the upright's top: convex, each
            // face receding from its line. At y = 10 it lands at (400, 400 − 240/√6) =
            // (400, 302.02), 30° below the row as Edge1 is: s = 0.866 (row − 302.02) − 0.5
            // (column − 400) at a pixel's centre, the wall's side positive. Within the band,
            // the edge; 12 pixels off (12.29 below, 11.96 above), the square misses its stroke
            // (it comes within 4.09 and 3.76 pixels of the line) and the wall below or the
            // top above is taken. The upright's other edge, (0, y, 20), is 28 pixels above it.
            let mesh = Arc::new(l_mesh());
            r.upload(1, &mesh).unwrap();
            let camera = l_camera();
            assert_pixel(
                project(&camera, SQUARE, [5.0, 10.0, 20.0]),
                [400.0, 400.0 - 240.0 / 6f64.sqrt()],
                1e-4,
            );
            for (at, want) in [
                ([400, 302], Edge(4)),
                ([400, 304], Edge(4)),
                ([400, 300], Edge(4)),
                ([400, 316], Face(2)),
                ([400, 288], Face(3)),
            ] {
                let picking = pick(r, &square_frame(&camera, &[1], at));
                assert_eq!(taken(&picking, at), Some((1, want)), "{at:?}");
            }
            // Its own pixel's centre, 0.17 pixels off its line: the edge, its point the drawn
            // line's, its reach undone.
            let picking = pick(r, &square_frame(&camera, &[1], [400, 302]));
            let hit = picking.hit.unwrap();
            assert_eq!((hit.body, hit.hit), (1, Edge(4)));
            let point = hit.point.unwrap();
            let off = from_line(point, [5.0, 0.0, 20.0], [0.0, 1.0, 0.0]);
            assert!(off < 0.03, "{point:?} is {off} off Edge4");
        },
    );
}

#[test]
fn the_casts_first_at_a_concave_edge_is_what_the_renderer_picks_within_its_reach() {
    on_lavapipe(
        "render::frame_tests::the_casts_first_at_a_concave_edge_is_what_the_renderer_picks_within_its_reach",
        |r| {
            use Element::{Edge, Face};
            // At each pixel's centre the cast lists Edge1 by its depth less its reach, 0.25 mm,
            // the face under the pointer by its own, 0.153 mm a pixel off the line nearer the
            // eye: so the edge first while that's under 0.25, 1.63 pixels off, and never
            // hidden (on the ray through its own point both faces meet it at its depth). The
            // renderer takes the edge from the square: they agree at its own pixel and 1 or
            // so either side; 1.92 off, the floor is 0.294 mm nearer and comes first, the
            // edge after it, where the hover takes the edge (the square reaches the stroke).
            let mesh = Arc::new(l_mesh());
            r.upload(1, &mesh).unwrap();
            let pickable = Pickable::new(Arc::clone(&mesh));
            let camera = l_camera();
            for (at, first) in [
                ([400, 400], Edge(1)),
                ([400, 401], Edge(1)),
                ([400, 399], Edge(1)),
                ([400, 398], Edge(1)),
                ([400, 402], Face(1)),
            ] {
                let gpu = taken(&pick(r, &square_frame(&camera, &[1], at)), at);
                let pointer = at.map(|c| f64::from(c) + 0.5);
                let cast = hits(&camera, SQUARE, 1.0, &[(1, &pickable)], pointer);
                assert_eq!(gpu, Some((1, Edge(1))), "{at:?}");
                let head = cast.first().map(|h| (h.body, h.element, h.hidden));
                assert_eq!(head, Some((1, first, false)), "{at:?}: {cast:?}");
                let edge = cast.iter().find(|h| h.element == Edge(1)).unwrap();
                assert!(!edge.hidden, "{at:?}: {cast:?}");
            }
        },
    );
}

/// A plate 40 by 40 by 1 from the origin, and Edge13 across its middle along x at height `zb`,
/// (10, 20, zb)–(30, 20, zb): at the plate's bottom (zb = 0) it's 1 mm behind its top,
/// Face6 (z = 1), seen from above.
fn thin_wall(zb: f64) -> Arc<Mesh> {
    let mut mesh = box_mesh([0.0; 3], [40.0, 40.0, 1.0]);
    mesh.edge_points
        .extend([[10.0, 20.0, zb], [30.0, 20.0, zb]]);
    mesh.edge_point_counts.push(2);
    Arc::new(mesh)
}

/// From (20, 20, 101) straight down onto the plate's top, +y up, 800 by 800: orthographic and
/// 100 mm high (0.125 mm a pixel), or in perspective 2 atan 0.5 = 53.13° high, so the view
/// is 1 mm high for each mm from the eye, 100 mm at the top's depth. Either way Edge13 lies
/// along row 400.0, from column 320 to 480, and the pixel (400, 400) has its centre half a
/// pixel off it; the plate's outline is 160 pixels away.
fn above_the_wall(perspective: bool) -> Camera {
    Camera {
        projection: if perspective {
            Projection::Perspective {
                field_of_view: (2.0 * libm::atan(0.5)).to_degrees(),
            }
        } else {
            Projection::Orthographic { view_height: 100.0 }
        },
        eye: [20.0, 20.0, 101.0],
        target: [20.0, 20.0, 1.0],
        up: [0.0, 1.0, 0.0],
    }
}

#[test]
fn an_edge_behind_a_thin_wall_is_picked_only_within_its_reach_of_the_walls_face() {
    on_lavapipe(
        "render::frame_tests::an_edge_behind_a_thin_wall_is_picked_only_within_its_reach_of_the_walls_face",
        |r| {
            use Element::{Edge, Face};
            let at = [400, 400];
            let pointer = [400.5, 400.5];
            // Orthographic: the edge's reach is 2 pixels of depth, 0.25 mm (the camera 28.30 +
            // 1 mm in front of the target, the plate's corner (0, 0, 0) √801 from it, so its
            // cap, half of 30.30 or so, is far off). 0.2 mm behind the top it's picked, 0.3
            // behind it isn't, nor 1 mm (at the bottom), which a vertex's reach, 1.2 mm,
            // would have taken. The cast marks it hidden where it isn't picked.
            // In perspective: the reach is 2/800 of its depth along its line to the eye, so
            // it's moved from 101 − zb to (101 − zb) × 0.9975, against the top at 100:
            // 100.748 at the bottom, behind; 99.950 at 0.8, in front; 100.049 at 0.7, behind.
            for perspective in [false, true] {
                let camera = above_the_wall(perspective);
                // Edge13's end at 0.8: orthographic, 8 pixels a millimetre left of the
                // centre; in perspective, 100.2 from the eye, where a pixel is 100.2/800 mm.
                let column = if perspective {
                    400.0 - 10.0 * 800.0 / 100.2
                } else {
                    320.0
                };
                assert_pixel(
                    project(&camera, SQUARE, [10.0, 20.0, 0.8]),
                    [column, 400.0],
                    1e-4,
                );
                for (zb, want) in [(0.0, Face(6)), (0.8, Edge(13)), (0.7, Face(6))] {
                    let mesh = thin_wall(zb);
                    r.upload(1, &mesh).unwrap();
                    let picking = pick(r, &square_frame(&camera, &[1], at));
                    let what = format!("zb {zb}, perspective {perspective}");
                    assert_eq!(taken(&picking, at), Some((1, want)), "{what}");
                    if want == Face(6) {
                        assert_eq!(picking.edge, None, "{what}");
                    }
                    let pickable = Pickable::new(Arc::clone(&mesh));
                    let cast = hits(&camera, SQUARE, 1.0, &[(1, &pickable)], pointer);
                    let edge = cast.iter().find(|h| h.element == Edge(13)).unwrap();
                    assert_eq!(edge.hidden, want == Face(6), "{what}: {cast:?}");
                    let head = cast.first().map(|h| (h.element, h.hidden));
                    assert_eq!(head, Some((want, false)), "{what}: {cast:?}");
                }
            }
        },
    );
}

#[test]
fn a_wall_a_millimetre_thick_in_front_of_another_bodys_edge_keeps_it_unpickable() {
    on_lavapipe(
        "render::frame_tests::a_wall_a_millimetre_thick_in_front_of_another_bodys_edge_keeps_it_unpickable",
        |r| {
            use Element::{Edge, Face};
            // Body 1 a block (10, 0, −10)–(30, 20, 0), its top's back edge Edge7 (30, 20, 0)–
            // (10, 20, 0) along row 400.0 of the view from above, 100 mm over 800 rows; body 2
            // the plate (0, 0, 0)–(40, 40, 1) lying on it, its top, Face6, 1 mm nearer the
            // eye. The edge's reach is 0.25 mm, so the plate's top hides it: the pixel (400,
            // 400), half a pixel off the edge's line, and its square give the plate's top and
            // no edge (the plate's outline, and the block's other edges, are 80 pixels or more
            // away); a vertex's reach, 1.2 mm, would have the block's edge through the plate.
            // The cast lists it hidden by the plate.
            let block = Arc::new(box_mesh([10.0, 0.0, -10.0], [20.0, 20.0, 10.0]));
            let plate = Arc::new(box_mesh([0.0; 3], [40.0, 40.0, 1.0]));
            r.upload(1, &block).unwrap();
            r.upload(2, &plate).unwrap();
            let camera = above_the_wall(false);
            assert_pixel(
                project(&camera, SQUARE, [30.0, 20.0, 0.0]),
                [480.0, 400.0],
                1e-4,
            );
            let at = [400, 400];
            let picking = pick(r, &square_frame(&camera, &[1, 2], at));
            assert_eq!(picking.edge, None);
            assert_eq!(taken(&picking, at), Some((2, Face(6))));
            let block = Pickable::new(block);
            let plate = Pickable::new(plate);
            let cast = hits(
                &camera,
                SQUARE,
                1.0,
                &[(1, &block), (2, &plate)],
                [400.5, 400.5],
            );
            let head = cast.first().map(|h| (h.body, h.element, h.hidden));
            assert_eq!(head, Some((2, Face(6), false)), "{cast:?}");
            let edge = cast
                .iter()
                .find(|h| (h.body, h.element) == (1, Edge(7)))
                .unwrap();
            assert!(edge.hidden, "{cast:?}");
        },
    );
}

#[test]
fn an_edges_picking_strip_draws_nothing_of_the_image() {
    on_lavapipe(
        "render::frame_tests::an_edges_picking_strip_draws_nothing_of_the_image",
        |r| {
            // The L at the frames' 64 by 48, 30 mm high about the concave edge's middle (0.625
            // mm a pixel): its reach is the vertex's 1.2% of 30, 0.36 mm (2 pixels of 48 rows
            // would be more), and the faces beside it rise 0.765 mm a pixel off its line, so
            // its strip wins within 0.47 pixels of it, the pixel (32, 24)'s centre 0.18 off
            // among them; a strip drawn into the image would show there, where its faces hide
            // its drawn stroke. A frame picking the whole frame, and one not picking, draw the
            // same pixels; and the picking found the edge.
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = Camera {
                projection: Projection::Orthographic { view_height: 30.0 },
                ..l_camera()
            };
            let whole = frame(&camera, &[1], &[], Some(rect([0, 0], [WIDTH, HEIGHT])));
            let commands = r.prepare(&whole).unwrap();
            let picked = composited(r, Some(commands));
            let commands = r
                .prepare(&Frame {
                    picking: None,
                    ..whole
                })
                .unwrap();
            assert!(composited(r, Some(commands)) == picked, "the image changed");
            let centre = frame(&camera, &[1], &[], Some(rect([32, 24], [1, 1])));
            assert_eq!(hit(r, &centre), Some((1, Element::Edge(1), [32, 24])));
        },
    );
}

/// A frame wider than it's high, 1200 by 800: at the same view height, 0.125 mm a pixel either
/// way about the centre (600, 400), so each scene's numbers are the square frame's 200 columns
/// right; but a reach taken of its 1200 columns, not its 800 rows, would be 2/1200 of the
/// view's height (0.167 mm at 100 mm high, against 2/800's 0.25), and shows.
const WIDE: [u32; 2] = [1200, 800];

#[test]
fn an_edges_reach_is_of_the_views_rows_in_a_frame_wider_than_it_is_high() {
    on_lavapipe(
        "render::frame_tests::an_edges_reach_is_of_the_views_rows_in_a_frame_wider_than_it_is_high",
        |r| {
            use Element::{Edge, Face};
            let mesh = Arc::new(l_mesh());
            r.upload(1, &mesh).unwrap();
            let pickable = Pickable::new(Arc::clone(&mesh));
            let camera = l_camera();
            assert_pixel(
                project(&camera, WIDE, [5.0, 10.0, 5.0]),
                [600.0, 400.0],
                1e-4,
            );
            assert_pixel(
                project(&camera, WIDE, [5.0, 20.0, 5.0]),
                [600.0 + 80.0 / 2f64.sqrt(), 400.0 + 80.0 / 6f64.sqrt()],
                1e-4,
            );
            // The L's concave corner, its pixels' signed distances from Edge1's image those of
            // the square frame's (s = 0.866 (row − 400) − 0.5 (column − 600) at a centre). The
            // renderer takes the edge within the band and the face past it. The cast lists the
            // edge first while the face under the pointer is nearer the eye than its line by
            // less than its reach, 0.25 mm: 0.153 mm a pixel off it, so 1.55 pixels off on the
            // wall's side (0.237 mm) the edge (a reach of 0.167 would put the wall first), and
            // 1.92 off on the floor's (0.294 mm) the floor.
            for (at, s, gpu, first) in [
                ([600, 400], 0.18, Edge(1), Some(Edge(1))),
                ([600, 401], 1.05, Edge(1), Some(Edge(1))),
                ([600, 399], -0.68, Edge(1), Some(Edge(1))),
                ([600, 398], -1.55, Edge(1), Some(Edge(1))),
                ([600, 402], 1.92, Edge(1), Some(Face(1))),
                ([600, 412], 10.58, Face(1), None),
                ([600, 388], -10.21, Face(2), None),
            ] {
                let centre = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 0.5];
                let off = 0.75f64.sqrt() * (centre[1] - 400.0) - 0.5 * (centre[0] - 600.0);
                assert!((off - s).abs() < 0.01, "{at:?}: {off}");
                let picking = pick(r, &band_frame(&camera, &[1], at, WIDE));
                assert_eq!(taken(&picking, at), Some((1, gpu)), "{at:?}");
                if let Some(first) = first {
                    let cast = hits(&camera, WIDE, 1.0, &[(1, &pickable)], centre);
                    let head = cast.first().map(|h| (h.body, h.element, h.hidden));
                    assert_eq!(head, Some((1, first, false)), "{at:?}: {cast:?}");
                }
            }
            // The edge's own pixel, its point the drawn line's, its reach of 0.25 mm undone,
            // by hand as in the square frame: (5 + across, 10 + along, 5 − across). Undone by
            // 0.167, it would be 0.083 mm along the view, toward the eye, from it.
            let picking = pick(r, &band_frame(&camera, &[1], [600, 400], WIDE));
            let hit = picking.hit.unwrap();
            assert_eq!((hit.body, hit.hit, hit.pixel), (1, Edge(1), [600, 400]));
            let half = 0.75f64.sqrt() / 2.0;
            let along = (half + 0.25) / (8.0 * (2.0f64 / 3.0).sqrt());
            let across = (half - 0.25) * 0.125 / 2f64.sqrt();
            assert_point(&hit, [5.0 + across, 10.0 + along, 5.0 - across], [2e-3; 3]);
            // The thin wall, its edge 0.2 and 0.3 mm behind the plate's top, along row 400 from
            // column 520 to 680 orthographic. Orthographic, the reach is 0.25 mm: picked at 0.2
            // behind, not at 0.3 (0.167 would pick neither). In perspective it's 2/800 of the
            // depth along the line to the eye, moving the edge from 100.2 to 99.950 and from
            // 100.3 to 100.049, against the top at 100 (2/1200 would move it to 100.033 and
            // 100.133, both behind). The cast marks it hidden where it isn't picked. Where it's
            // picked, its point is its own: z = 0.8, at the pixel's centre half a pixel along
            // and across it (0.125 mm a pixel orthographic; in perspective 100.2/800 there).
            // Its reach undone by 0.167 would leave z at 0.883 orthographic, and 0.884.
            for perspective in [false, true] {
                let camera = above_the_wall(perspective);
                let pixel = if perspective { 100.2 / 800.0 } else { 0.125 };
                assert_pixel(
                    project(&camera, WIDE, [10.0, 20.0, 0.8]),
                    [600.0 - 10.0 / pixel, 400.0],
                    1e-4,
                );
                let at = [600, 400];
                for (zb, want) in [(0.8, Edge(13)), (0.7, Face(6))] {
                    let mesh = thin_wall(zb);
                    r.upload(1, &mesh).unwrap();
                    let what = format!("zb {zb}, perspective {perspective}");
                    let picking = pick(r, &band_frame(&camera, &[1], at, WIDE));
                    assert_eq!(taken(&picking, at), Some((1, want)), "{what}");
                    let pickable = Pickable::new(Arc::clone(&mesh));
                    let cast = hits(&camera, WIDE, 1.0, &[(1, &pickable)], [600.5, 400.5]);
                    let edge = cast.iter().find(|h| h.element == Edge(13)).unwrap();
                    assert_eq!(edge.hidden, want == Face(6), "{what}: {cast:?}");
                    let head = cast.first().map(|h| (h.element, h.hidden));
                    assert_eq!(head, Some((want, false)), "{what}: {cast:?}");
                    if want == Edge(13) {
                        let hit = picking.hit.unwrap();
                        assert_eq!((hit.hit, hit.pixel), (Edge(13), at), "{what}");
                        let want = [20.0 + pixel / 2.0, 20.0 - pixel / 2.0, 0.8];
                        assert_point(&hit, want, [1e-3; 3]);
                    }
                }
            }
        },
    );
}

#[test]
fn a_renders_image_and_id_buffer_have_a_concave_edges_stroke_where_its_faces_hid_it() {
    on_lavapipe(
        "render::frame_tests::a_renders_image_and_id_buffer_have_a_concave_edges_stroke_where_its_faces_hid_it",
        |r| {
            use Element::Edge;
            // The worker's render of the L at the study's 800 by 800: its edges drawn as a
            // frame's are, toward the eye by √2 + 1/2 = 1.91 pixels of depth, and no edge drawn
            // again for picking. The pixel (400, 400), its centre 0.18 pixels off Edge1 on the
            // floor's side, where the floor is 0.18 × 1.2247 = 0.22 pixels of depth nearer the
            // eye than its line, and (400, 399), 0.68 off on the wall's side (0.84 nearer),
            // are within its stroke (the ID buffer takes a pixel whose centre it covers half of
            // or more, to 0.99 off): Edge1's in the ID buffer, where its faces had them before,
            // and (400, 400) the edges' colour in the image. (400, 401), 1.05 off, and
            // (400, 398), 1.55 off, past its stroke, are the floor's, Face1, and the wall's,
            // Face2. A frame picking each of the first two alone takes Edge1 there too.
            let scene = silico_protocol::RenderScene {
                bodies: vec![silico_protocol::RenderBody {
                    body: 1,
                    mesh: l_mesh(),
                    highlight_faces: vec![],
                    highlight_edges: vec![],
                }],
                camera: l_camera(),
                width: SQUARE[0],
                height: SQUARE[1],
                ids: true,
            };
            scene.check().unwrap();
            let rendered = r.render(&scene).unwrap();
            let ids = rendered.ids.unwrap();
            let id = |[x, y]: [u32; 2]| ids[usize::try_from(y * SQUARE[0] + x).unwrap()];
            let edge = silico_protocol::EDGE_BIT | 1;
            for (at, want) in [
                ([400, 400], edge),
                ([400, 399], edge),
                ([400, 401], 1),
                ([400, 398], 2),
            ] {
                assert_eq!(id(at), [1, want], "{at:?}");
            }
            let mut reader = png::Decoder::new(std::io::Cursor::new(rendered.png))
                .read_info()
                .unwrap();
            let mut rgb = vec![0; reader.output_buffer_size().unwrap()];
            reader.next_frame(&mut rgb).unwrap();
            let at = usize::try_from(3 * (400 * SQUARE[0] + 400)).unwrap();
            let pixel = [rgb[at], rgb[at + 1], rgb[at + 2], 255];
            assert!(edge_coloured(pixel, pixel), "(400, 400): {pixel:?}");
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = l_camera();
            for at in [[400, 400], [400, 399]] {
                let asking = Frame {
                    picking: Some(rect(at.map(|c| i32::try_from(c).unwrap()), [1, 1])),
                    ..square_frame(&camera, &[1], at)
                };
                assert_eq!(hit(r, &asking), Some((1, Edge(1), at)), "{at:?}");
            }
        },
    );
}

#[test]
fn with_edges_not_picked_a_concave_edges_stroke_is_picked_where_its_drawn_and_past_it_its_faces() {
    on_lavapipe(
        "render::frame_tests::with_edges_not_picked_a_concave_edges_stroke_is_picked_where_its_drawn_and_past_it_its_faces",
        |r| {
            use Element::{Edge, Face};
            // The selection's edge filter off (E, or a dialog's box taking faces): a frame that
            // picks draws no edge's picking strip, so it picks each edge's drawn strip, moved
            // toward the eye by 1.91 pixels of depth (0.239 mm), where it's drawn: a concave
            // edge's as a convex one's. The pixels (400, 400) and (400, 399), 0.18 and 0.68 off
            // Edge1 (its faces 0.22 and 0.84 pixels of depth nearer the eye there), are Edge1's,
            // so the selection, taking faces alone, takes nothing there, as on a convex edge's
            // stroke. Past its stroke, (400, 401), 1.05 off on the floor's side, is the floor's,
            // Face1, its point on it (z = 5), and (400, 398), 1.55 off on the wall's side, the
            // wall's, Face2 (x = 5): there the selection takes the face under the pointer.
            let mesh = Arc::new(l_mesh());
            r.upload(1, &mesh).unwrap();
            let camera = l_camera();
            let off = |at: [u32; 2]| Frame {
                edges: false,
                ..square_frame(&camera, &[1], at)
            };
            for at in [[400, 400], [400, 399]] {
                let picking = pick(r, &off(at));
                let found = picking.hit.map(|h| (h.body, h.hit, h.pixel));
                assert_eq!(found, Some((1, Edge(1), at)), "{at:?}");
            }
            for (at, face, k, plane) in
                [([400, 401], Face(1), 2, 5.0), ([400, 398], Face(2), 0, 5.0)]
            {
                let picking = pick(r, &off(at));
                let hit = picking.hit.unwrap();
                assert_eq!((hit.body, hit.hit, hit.pixel), (1, face, at), "{at:?}");
                let point = hit.point.unwrap();
                assert!((point[k] - plane).abs() < 1e-3, "{at:?}: {point:?}");
            }
        },
    );
}

// ---- Hostile edge picks (N5) ----

#[test]
fn a_crowd_of_edges_on_the_picking_square_is_drawn_again_only_near_it_and_only_when_picking() {
    on_lavapipe(
        "render::frame_tests::a_crowd_of_edges_on_the_picking_square_is_drawn_again_only_near_it_and_only_when_picking",
        |r| {
            // The front view's box, 100,000 edges across its front face's middle, (10, 0, 7)–
            // (30, 0, 7), along row 24.0 from column 20 to 41.82, and one off the frame,
            // (1000, 0, 7)–(1020, 0, 7), column 1100 on. A frame picking the pixel (32, 24),
            // half a pixel below their line, draws again the edges whose box lands within a
            // stroke (a point) and a pixel more of it, columns 30 to 35 and rows 22 to 27: the
            // 100,000, not the far one, nor any of the box's twelve (its top and bottom along
            // rows 20.73 and 31.64, its sides down columns 9.09 and 52.73); and picks one of
            // the crowd there. A frame not picking draws none again, nor one refused (its
            // camera's eye on its target), nor one picking a pixel far off the frame.
            let mut mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            for _ in 0..100_000 {
                mesh.edge_points
                    .extend([[10.0, 0.0, 7.0], [30.0, 0.0, 7.0]]);
                mesh.edge_point_counts.push(2);
            }
            mesh.edge_points
                .extend([[1000.0, 0.0, 7.0], [1020.0, 0.0, 7.0]]);
            mesh.edge_point_counts.push(2);
            r.upload(1, &Arc::new(mesh)).unwrap();
            let camera = front();
            let drawn = || super::EDGE_PICKS_DRAWN.with(std::cell::Cell::get);
            let before = drawn();
            draw(r, &frame(&camera, &[1], &[], None));
            assert_eq!(drawn(), before, "a frame not picking");
            let mut broken = camera.clone();
            broken.eye = broken.target;
            let centre = Some(rect([32, 24], [1, 1]));
            assert!(matches!(
                r.prepare(&frame(&broken, &[1], &[], centre)),
                Err(RenderFailure::Invalid(_))
            ));
            assert_eq!(drawn(), before, "a frame refused");
            assert_eq!(r.take_picking(), None);
            let far = Some(rect([-(1 << 24), 1 << 24], [1, 1]));
            draw(r, &frame(&camera, &[1], &[], far));
            assert_eq!(drawn(), before, "a frame picking far off it");
            let found = hit(r, &frame(&camera, &[1], &[], centre));
            assert_eq!(drawn(), before + 100_000, "the frame picking the crowd");
            assert!(
                matches!(found, Some((1, Element::Edge(13..=100_012), [32, 24]))),
                "{found:?}"
            );
            // Still drawn, and still the face away from them.
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(rect([32, 28], [1, 1])))),
                Some((1, Element::Face(3), [32, 28]))
            );
        },
    );
}

#[test]
fn a_concave_edge_as_far_out_as_the_renderer_draws_is_picked_as_one_at_the_origin_is() {
    on_lavapipe(
        "render::frame_tests::a_concave_edge_as_far_out_as_the_renderer_draws_is_picked_as_one_at_the_origin_is",
        |r| {
            use Element::{Edge, Face};
            // The L and its camera moved 10^12 − 200 mm along x and y, the eye just inside the
            // renderer's coordinates: the picks of the L at the origin, the strips placed about
            // the camera in f64; the edge's point the moved one's, to f64's step there (1.2e-4
            // mm) and the hand's 2e-3.
            let t = silico_protocol::MAX_RENDER_COORDINATE - 200.0;
            let moved = |p: &[f64; 3]| [p[0] + t, p[1] + t, p[2]];
            let mut mesh = l_mesh();
            mesh.positions = mesh.positions.iter().map(moved).collect();
            mesh.edge_points = mesh.edge_points.iter().map(moved).collect();
            mesh.vertex_points = mesh.vertex_points.iter().map(moved).collect();
            r.upload(1, &Arc::new(mesh)).unwrap();
            let home = l_camera();
            let camera = Camera {
                eye: moved(&home.eye),
                target: moved(&home.target),
                ..home
            };
            for (at, want) in [
                ([400, 400], Edge(1)),
                ([400, 402], Edge(1)),
                ([400, 398], Edge(1)),
                ([400, 412], Face(1)),
                ([400, 388], Face(2)),
            ] {
                let picking = pick(r, &square_frame(&camera, &[1], at));
                assert_eq!(taken(&picking, at), Some((1, want)), "{at:?}");
            }
            let picking = pick(r, &square_frame(&camera, &[1], [400, 400]));
            let hit = picking.hit.unwrap();
            assert_eq!((hit.body, hit.hit), (1, Edge(1)));
            let half = 0.75f64.sqrt() / 2.0;
            let along = (half + 0.25) / (8.0 * (2.0f64 / 3.0).sqrt());
            let across = (half - 0.25) * 0.125 / 2f64.sqrt();
            assert_point(
                &hit,
                [5.0 + across + t, 10.0 + along + t, 5.0 - across],
                [2e-3; 3],
            );
        },
    );
}

#[test]
fn an_edge_as_long_as_the_renderer_draws_across_a_perspective_view_is_cut_then_moved_and_picked() {
    on_lavapipe(
        "render::frame_tests::an_edge_as_long_as_the_renderer_draws_across_a_perspective_view_is_cut_then_moved_and_picked",
        |r| {
            // From (21, −100, 7) along +y, 60° high: the front face square on at depth 100,
            // and Edge13 along x on it at z = 7, from −10^12 to 10^12, the renderer's farthest:
            // along row 24.0 through the view's centre, cut to the view's pyramid (2^19 pixels
            // off its axis) as its drawn strip is, then moved toward the eye by its reach (at
            // 48 rows a vertex's, 1.2% of 2 tan 30° of its depth: 1.386 mm at 100). The pixel
            // (32, 24), half a pixel off its line, is the edge's, its point on the face (y =
            // 0) to f32's depth there, the reach undone (without the undoing, y = −1.386); and
            // the frame draws.
            let far = silico_protocol::MAX_RENDER_COORDINATE;
            let mut mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            mesh.edge_points.extend([[-far, 0.0, 7.0], [far, 0.0, 7.0]]);
            mesh.edge_point_counts.push(2);
            r.upload(1, &Arc::new(mesh)).unwrap();
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 60.0,
                },
                eye: [21.0, -100.0, 7.0],
                target: [21.0, 0.0, 7.0],
                up: [0.0, 0.0, 1.0],
            };
            assert_pixel(
                project(&camera, [WIDTH, HEIGHT], [21.0, 0.0, 7.0]),
                [32.0, 24.0],
                1e-9,
            );
            let picked = pick(r, &frame(&camera, &[1], &[], Some(rect([32, 24], [1, 1]))));
            let hit = picked.hit.unwrap();
            assert_eq!(
                (hit.body, hit.hit, hit.pixel),
                (1, Element::Edge(13), [32, 24])
            );
            let point = hit.point.unwrap();
            assert!(point[1].abs() < 1e-3, "{point:?}");
        },
    );
}

#[test]
fn an_edge_is_drawn_again_for_picking_within_half_its_stroke_and_a_pixel_of_the_rectangle_and_no_farther()
 {
    on_lavapipe(
        "render::frame_tests::an_edge_is_drawn_again_for_picking_within_half_its_stroke_and_a_pixel_of_the_rectangle_and_no_farther",
        |r| {
            // The front view's box and two edges across its front face, (10, 0, z)–(30, 0, z),
            // columns 20 to 41.82: at z = 4.5 along row 24 + 2.5 × 12/11 = 26.73, and at z = 4
            // along row 24 + 3 × 12/11 = 27.27. At a pixel a point, a frame picking the pixel
            // (32, 24), rows 24 to 25, draws again the edges whose box lands within half their
            // stroke (a point, a pixel) and a pixel more of it: rows 22 to 27, columns 30 to 35.
            // So the first, 0.27 inside, and not the second, 0.27 past, nor any of the box's
            // twelve (along rows 20.73 and 31.64, down columns 9.09 and 52.73, or at their
            // corners); picking the pixel below, rows 23 to 28, both. A margin a pixel wider
            // (rows 21 to 28) would take both at once; one of the pixel alone (rows 23 to 26,
            // then 24 to 27), neither, then only the first.
            let mut mesh = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
            for z in [4.5, 4.0] {
                mesh.edge_points.extend([[10.0, 0.0, z], [30.0, 0.0, z]]);
                mesh.edge_point_counts.push(2);
            }
            r.upload(1, &Arc::new(mesh)).unwrap();
            let camera = front();
            for (z, row) in [(4.5, 24.0 + 30.0 / 11.0), (4.0, 24.0 + 36.0 / 11.0)] {
                assert_pixel(
                    project(&camera, [WIDTH, HEIGHT], [21.0, 0.0, z]),
                    [32.0, row],
                    1e-6,
                );
            }
            let drawn = || super::EDGE_PICKS_DRAWN.with(std::cell::Cell::get);
            for (row, more) in [(24, 1), (25, 2)] {
                let before = drawn();
                draw(r, &frame(&camera, &[1], &[], Some(rect([32, row], [1, 1]))));
                assert_eq!(drawn(), before + more, "picking row {row}");
            }
        },
    );
}

#[test]
fn at_four_pixels_a_point_a_concave_edge_is_picked_two_and_a_half_pixels_off_its_line() {
    on_lavapipe(
        "render::frame_tests::at_four_pixels_a_point_a_concave_edge_is_picked_two_and_a_half_pixels_off_its_line",
        |r| {
            use Element::{Edge, Face};
            // The L from (45, 10, 85) about the concave edge's middle, (5, 10, 5), z up,
            // orthographic and 100 mm high over 800 rows: 0.125 mm a pixel. Its axes by hand:
            // back (1, 0, 2)/√5, up (−2, 0, 1)/√5, right up × back = (0, 1, 0); a point lands at
            // column 400 + 8 (y − 10) and row 400 + 8 (2 (x − 5) − (z − 5))/√5. Edge1,
            // (5, y, 5), lies along row 400 from column 320 to 480, and the floor (Face1, z = 5)
            // below it, a millimetre of x 16/√5 pixels down and 1/√5 mm toward the eye: half a
            // pixel of depth a pixel. The pixel (400, 402), its centre 2.5 pixels below the
            // line, is inside the stroke's half width at 4 pixels a point (a point, 4 pixels),
            // and the floor there is 1.25 pixels of depth nearer the eye than the line: it hid
            // the drawn stroke before N5's image, and not the picking strip, 2 pixels of depth
            // toward the eye, as the drawn strip is now at 4 pixels a point too. So the pixel
            // is the edge's at 4 pixels a point, and the floor's at 1, whose 1-pixel stroke
            // misses it. The strip is drawn for a rectangle its box lands within half
            // the stroke and a pixel of, rows 397 to 408 at 4 pixels a point; within a quarter
            // of a pixel and a pixel (rows 400.75 to 404.25), or a pixel alone, it would be left
            // out. The L's other edges along y lie along rows 382.1 (x and z 0) and farther,
            // and those across it down columns 320 and 480: the strip is Edge1's alone.
            let mesh = Arc::new(l_mesh());
            r.upload(1, &mesh).unwrap();
            let camera = Camera {
                projection: Projection::Orthographic { view_height: 100.0 },
                eye: [45.0, 10.0, 85.0],
                target: [5.0, 10.0, 5.0],
                up: [0.0, 0.0, 1.0],
            };
            let r5 = 5f64.sqrt();
            for (p, want) in [
                ([5.0, 10.0, 5.0], [400.0, 400.0]),
                ([5.0, 20.0, 5.0], [480.0, 400.0]),
                ([7.0, 10.0, 5.0], [400.0, 400.0 + 32.0 / r5]),
                ([0.0, 10.0, 0.0], [400.0, 400.0 - 40.0 / r5]),
            ] {
                assert_pixel(project(&camera, SQUARE, p), want, 1e-4);
            }
            let at = [400, 402];
            let picking = |ppp: f32| Frame {
                size_px: SQUARE,
                pixels_per_point: ppp,
                ..frame(&camera, &[1], &[], Some(rect([400, 402], [1, 1])))
            };
            let drawn = || super::EDGE_PICKS_DRAWN.with(std::cell::Cell::get);
            let before = drawn();
            assert_eq!(hit(r, &picking(4.0)), Some((1, Edge(1), at)));
            assert_eq!(drawn(), before + 1);
            assert_eq!(hit(r, &picking(1.0)), Some((1, Face(1), at)));
        },
    );
}

#[test]
fn a_body_behind_a_perspective_eye_has_no_edge_drawn_again_for_picking_though_its_box_meets_the_rectangle()
 {
    on_lavapipe(
        "render::frame_tests::a_body_behind_a_perspective_eye_has_no_edge_drawn_again_for_picking_though_its_box_meets_the_rectangle",
        |r| {
            // From (0, −100, 0) along +y, 90° high, 64 by 48: a point (x, y, z) at depth
            // d = y + 100 lands at column 32 + 24 x / d and row 24 − 24 z / d. Body 2, an edge
            // (−10, 0, 0)–(10, 0, 0), lies along row 24 from column 29.6 to 34.4, through the
            // picking pixel (32, 24). Body 1, an edge (−100, −110, −100)–(100, −110, 100), 10 mm
            // behind the eye, is wholly outside the frustum its stroke reaches the image from
            // (whose depths are above 0). The rectangle a frame's edges are drawn again within,
            // at a pixel a point columns 30 to 35 and rows 22 to 27, has its sides, times the
            // depth, 24 x + 2 d, 3 d − 24 x, 2 d − 24 z, and 24 z + 3 d, each at least 0 inside;
            // behind the eye their signs turn, and a corner of body 1's box (d = −10, x and z
            // ±100) gives each 2370 or more, so that test alone takes it. Only the frustum keeps
            // it out: the edge drawn again is body 2's alone, and it's picked.
            let edge = |a: [f64; 3], b: [f64; 3]| {
                Arc::new(Mesh {
                    positions: vec![],
                    normals: vec![],
                    triangles: vec![],
                    face_triangles: vec![],
                    edge_points: vec![a, b],
                    edge_point_counts: vec![2],
                    vertex_points: vec![],
                    valid: None,
                })
            };
            let (low, high) = ([-100.0, -110.0, -100.0], [100.0, -110.0, 100.0]);
            r.upload(1, &edge(low, high)).unwrap();
            r.upload(2, &edge([-10.0, 0.0, 0.0], [10.0, 0.0, 0.0]))
                .unwrap();
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 90.0,
                },
                eye: [0.0, -100.0, 0.0],
                target: [0.0; 3],
                up: [0.0, 0.0, 1.0],
            };
            let size = [WIDTH, HEIGHT];
            assert_pixel(
                project(&camera, size, [10.0, 0.0, -5.0]),
                [34.4, 25.2],
                1e-9,
            );
            assert_eq!(project(&camera, size, high), None);
            let centre = rect([32, 24], [1, 1]);
            let projector = super::Projector::new(&camera, size).unwrap();
            assert!(
                super::Near::of(centre, 1.0)
                    .sides(&projector)
                    .meets_box((low, high))
            );
            let drawn = || super::EDGE_PICKS_DRAWN.with(std::cell::Cell::get);
            let before = drawn();
            assert_eq!(
                hit(r, &frame(&camera, &[1, 2], &[], Some(centre))),
                Some((2, Element::Edge(1), [32, 24]))
            );
            assert_eq!(drawn(), before + 1);
        },
    );
}

// Lines among the bodies (Frame::lines: a shown sketch's curves, the approved prototype's
// `syncSketchLines`). Each expected pixel is placed by the module's rule, by hand, and
// checked against `project`; the colour is magenta, which neither the body's grey shading
// nor its dark edges can be, 1.5 points wide (the prototype's), so a line through a pixel's
// centre covers it whole (re_renderer's coverage is 1 within its half width less half a
// pixel) and the rows a pixel off only partly.

const MAGENTA: [u8; 4] = [255, 0, 255, 255];

/// [`VIEWPORT_BG`] as a pixel of the frame: opaque black.
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// z of row 26.5, a pixel's centre on the front face: 7 − 2.5 × 11/12.
const ROW_26_Z: f64 = 7.0 - 2.5 * 11.0 / 12.0;

/// A line along x from 2 to 38 at `y` and `z`: columns 32 + (2 − 21) × 12/11 = 11.27 to
/// 32 + 17 × 12/11 = 50.55, inside the front face's 9.09 to 52.73, so a line behind the box
/// is behind it at every pixel it could reach.
fn line_at(y: f64, z: f64) -> super::Strips {
    super::Strips::new([vec![[2.0, y, z], [38.0, y, z]]]).unwrap()
}

fn magenta(strips: &super::Strips) -> super::Lines<'_> {
    super::Lines {
        strips,
        colour: MAGENTA,
        width: 1.5,
    }
}

/// `frame` drawn and composited: its pixels, rows top to bottom.
fn image(r: &mut Renderer, frame: &Frame<'_>) -> Vec<[u8; 4]> {
    let commands = r.prepare(frame).unwrap();
    composited(r, Some(commands))
}

fn pixel(image: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    image[usize::try_from(y * WIDTH + x).unwrap()]
}

/// Whether `got` is within `levels` of `want` in every channel.
fn within(got: [u8; 4], want: [u8; 4], levels: u8) -> bool {
    got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= levels)
}

/// Row `row`'s pixels from column `from` to `to`, each within 8 levels of magenta.
fn assert_magenta_row(image: &[[u8; 4]], row: u32, from: u32, to: u32) {
    for x in from..=to {
        let got = pixel(image, x, row);
        assert!(within(got, MAGENTA, 8), "({x}, {row}): {got:?}");
    }
}

#[test]
fn a_line_on_the_front_face_is_drawn_where_project_puts_it_and_nowhere_else() {
    on_lavapipe(
        "render::frame_tests::a_line_on_the_front_face_is_drawn_where_project_puts_it_and_nowhere_else",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // By the module's rule, (21, 0, 4.708333) lands at (32, 26.5): row 24 + 2.5 (to
            // the f32 rounding of the matrix `project` takes as the GPU does).
            assert_pixel(
                project(&camera, [WIDTH, HEIGHT], [21.0, 0.0, ROW_26_Z]),
                [32.0, 26.5],
                1e-6,
            );
            let line = line_at(0.0, ROW_26_Z);
            let without = image(r, &frame(&camera, &[1], &[], None));
            let with = image(
                r,
                &Frame {
                    lines: &[magenta(&line)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            // Without it, the face's shade is there, not magenta.
            assert!(!within(pixel(&without, 32, 26), MAGENTA, 8));
            assert_magenta_row(&with, 26, 12, 49);
            // Rows 25 and 27 are a pixel's centre 1 from the line, inside its smoothing;
            // every other pixel is the frame without it, byte for byte.
            for y in (0..HEIGHT).filter(|y| !(25..=27).contains(y)) {
                for x in 0..WIDTH {
                    assert_eq!(pixel(&with, x, y), pixel(&without, x, y), "({x}, {y})");
                }
            }
        },
    );
}

#[test]
fn a_line_behind_the_box_is_hidden_and_one_in_front_of_it_drawn() {
    on_lavapipe(
        "render::frame_tests::a_line_behind_the_box_is_hidden_and_one_in_front_of_it_drawn",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let without = image(r, &frame(&camera, &[1], &[], None));
            // Half a millimetre behind the back face (y = 20): every pixel the frame's own.
            let behind = line_at(20.5, ROW_26_Z);
            let hidden = image(
                r,
                &Frame {
                    lines: &[magenta(&behind)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            assert!(hidden == without, "a line behind the box shows");
            // Half a millimetre in front of the front face: drawn as on it.
            let ahead = line_at(-0.5, ROW_26_Z);
            let drawn = image(
                r,
                &Frame {
                    lines: &[magenta(&ahead)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            assert_magenta_row(&drawn, 26, 12, 49);
        },
    );
}

#[test]
fn a_line_over_the_background_and_one_on_an_edge_are_drawn_over_them() {
    on_lavapipe(
        "render::frame_tests::a_line_over_the_background_and_one_on_an_edge_are_drawn_over_them",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            // Above the box: z = 7 + 8.5 × 11/12 lands on row 24 − 8.5 = 15.5.
            let z = 7.0 + 8.5 * 11.0 / 12.0;
            assert_pixel(
                project(&camera, [WIDTH, HEIGHT], [21.0, 0.0, z]),
                [32.0, 15.5],
                1e-6,
            );
            let above = line_at(0.0, z);
            let over = image(
                r,
                &Frame {
                    lines: &[magenta(&above)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            assert_eq!(
                pixel(&image(r, &frame(&camera, &[1], &[], None)), 32, 15),
                BLACK
            );
            assert_magenta_row(&over, 15, 12, 49);
            // With no background, as the app's frames: composited over what the pass holds
            // (green here), the line as opaque as over a background of its own, and the pass's
            // own colour beside it, above the box (row 5) and between it and the box (row 18).
            let commands = r
                .prepare(&Frame {
                    lines: &[magenta(&above)],
                    background: NO_BACKGROUND,
                    ..frame(&camera, &[1], &[], None)
                })
                .unwrap();
            let green = wgpu::Color {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0,
            };
            let bare = composited_over(r, Some(commands), green);
            assert_magenta_row(&bare, 15, 12, 49);
            for y in [5, 18] {
                assert_eq!(pixel(&bare, 32, y), [0, 255, 0, 255], "(32, {y})");
            }
            // Along the front face's top edge (Edge3, z = 10, row 20.73): the edge's dark
            // without it, the line drawn after the edges over it (the prototype's
            // `renderOrder` 4 against the edges' 2 and 3). Row 20's centre is 0.23 from the
            // line, inside its half width less half a pixel.
            let on_edge = line_at(0.0, 10.0);
            let edge = pixel(&image(r, &frame(&camera, &[1], &[], None)), 32, 20);
            assert!(!within(edge, MAGENTA, 8), "{edge:?}");
            let drawn = image(
                r,
                &Frame {
                    lines: &[magenta(&on_edge)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            assert_magenta_row(&drawn, 20, 12, 49);
            // And the edge is still picked under it.
            let edge_pick = pick(
                r,
                &Frame {
                    lines: &[magenta(&on_edge)],
                    ..frame(&camera, &[1], &[], Some(ON_THE_TOP_EDGE))
                },
            );
            let found = edge_pick.hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Edge(3), [32, 20])
            );
        },
    );
}

#[test]
fn a_line_is_picked_by_nothing_and_the_face_under_it_is_found_beside_it() {
    on_lavapipe(
        "render::frame_tests::a_line_is_picked_by_nothing_and_the_face_under_it_is_found_beside_it",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))),
                Some((1, Element::Face(3), [32, 26]))
            );
            // Row 26 is the line's: of the square's other pixels, (32, 25) and (32, 27) are
            // both 1.0 from its centre (32.5, 26.5), and the first in rows is taken.
            let line = line_at(0.0, ROW_26_Z);
            let picking = pick(
                r,
                &Frame {
                    lines: &[magenta(&line)],
                    ..frame(&camera, &[1], &[], Some(ON_THE_FACE))
                },
            );
            let found = picking.hit.unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Face(3), [32, 25])
            );
            // The face's point at (32.5, 25.5): (21 + 0.5 × 11/12, 0, 7 − 1.5 × 11/12).
            assert_point(
                &found,
                [21.0 + 5.5 / 12.0, 0.0, 7.0 - 16.5 / 12.0],
                [1e-3, 1e-3, 1e-3],
            );
            assert_eq!(picking.edge, None);
            // Over the background, the line alone: nothing.
            let above = line_at(0.0, 7.0 + 8.5 * 11.0 / 12.0);
            let picking = pick(
                r,
                &Frame {
                    lines: &[magenta(&above)],
                    ..frame(&camera, &[1], &[], Some(rect([30, 13], [5, 5])))
                },
            );
            assert_eq!((picking.hit, picking.edge), (None, None));
        },
    );
}

#[test]
fn an_orthographic_views_reach_takes_its_lines_so_a_line_with_no_body_is_drawn() {
    on_lavapipe(
        "render::frame_tests::an_orthographic_views_reach_takes_its_lines_so_a_line_with_no_body_is_drawn",
        |r| {
            let camera = front();
            // No body: a reach of 0 would set the camera 1 mm in front of the target (y = 9),
            // and the line at y = 0 would be behind it. Its box's corner reaches √(19² + 10² +
            // 2.2917²) = 21.593 mm from (21, 10, 7), the camera 1 mm past that.
            let line = line_at(0.0, ROW_26_Z);
            let lines = [magenta(&line)];
            let alone = Frame {
                lines: &lines,
                ..frame(&camera, &[], &[], None)
            };
            let below = 7.0 - ROW_26_Z;
            let reach = (19.0f64 * 19.0 + 100.0 + below * below).sqrt();
            assert!((reach - 21.593).abs() < 1e-3);
            let CheckedFrame { view, .. } = r.check_frame(&alone).unwrap();
            assert!((view.depth - (reach + 1.0)).abs() < 1e-12, "{}", view.depth);
            assert_magenta_row(&image(r, &alone), 26, 12, 49);
            // With the box, the box's reach, √(21² + 10² + 7²) = 24.290, is the farther.
            r.upload(1, &the_box()).unwrap();
            let boxed = Frame {
                lines: &lines,
                ..frame(&camera, &[1], &[], None)
            };
            let CheckedFrame { view, .. } = r.check_frame(&boxed).unwrap();
            let box_reach = (21.0f64 * 21.0 + 100.0 + 49.0).sqrt();
            assert!(
                (view.depth - (box_reach + 1.0)).abs() < 1e-12,
                "{}",
                view.depth
            );
            // A line 140 mm off, its corner √(119² + 10² + 2.2917²) = 119.441 mm away, sets it,
            // with a clearance of 1% of it; and Select Other's cast sets its near plane alike.
            let far =
                super::Strips::new([vec![[2.0, 0.0, ROW_26_Z], [140.0, 0.0, ROW_26_Z]]]).unwrap();
            let far_reach = (119.0f64 * 119.0 + 100.0 + below * below).sqrt();
            let CheckedFrame { view, .. } = r
                .check_frame(&Frame {
                    lines: &[magenta(&far)],
                    ..frame(&camera, &[1], &[], None)
                })
                .unwrap();
            let depth = far_reach * 1.01;
            assert!((view.depth - depth).abs() < 1e-9, "{}", view.depth);
            let body = Pickable::new(the_box());
            let cast =
                super::Caster::new(&camera, [WIDTH, HEIGHT], 1.0, &[(1, &body)], &[&far]).unwrap();
            assert!((cast.near.unwrap() - depth).abs() < 1e-9);
            let bare =
                super::Caster::new(&camera, [WIDTH, HEIGHT], 1.0, &[(1, &body)], &[]).unwrap();
            assert!((bare.near.unwrap() - (box_reach + 1.0)).abs() < 1e-12);
            // A line the view is too small for (44 mm over 48 rows takes a reach of at most
            // 44 × 2^19 / 48 = 480,597 mm) is refused, naming the lines, and the cast finds
            // nothing, as the frame draws nothing.
            let too_far = super::Strips::new([vec![[1e6, 10.0, 7.0], [1e6, 10.0, 8.0]]]).unwrap();
            assert_eq!(
                refused(
                    r,
                    &Frame {
                        lines: &[magenta(&too_far)],
                        ..frame(&camera, &[1], &[], None)
                    }
                ),
                "lines"
            );
            let centre = [32.5, 26.5];
            let size = [WIDTH, HEIGHT];
            assert!(!hits(&camera, size, 1.0, &[(1, &body)], centre).is_empty());
            assert!(
                super::hits_among(&camera, size, 1.0, &[(1, &body)], &[&too_far], centre)
                    .is_empty()
            );
        },
    );
}

#[test]
fn a_line_ten_kilometres_out_is_drawn_on_its_pixel_not_where_f32_would_put_it() {
    on_lavapipe(
        "render::frame_tests::a_line_ten_kilometres_out_is_drawn_on_its_pixel_not_where_f32_would_put_it",
        |r| {
            // From −y about (10^7, 10, 0.7), 4.4 mm over 48 rows (11/120 mm a pixel): x at
            // column 32 + (x − 10^7) × 120/11. A line along z at x = 10^7 + 1.2375 lands on
            // column 32 + 13.5 = 45.5, a pixel's centre. f32's step at 10^7 is 1 mm: in f32
            // world coordinates it would be 10^7 + 1, column 32 + 120/11 = 42.9.
            let camera = Camera {
                projection: Projection::Orthographic { view_height: 4.4 },
                eye: [1e7, -50.0, 0.7],
                target: [1e7, 10.0, 0.7],
                up: [0.0, 0.0, 1.0],
            };
            let x = 1e7 + 1.2375;
            assert_eq!(1e7_f32 + 1.2375, 1e7_f32 + 1.0);
            assert_pixel(
                project(&camera, [WIDTH, HEIGHT], [x, 10.0, 0.7]),
                [45.5, 24.0],
                1e-6,
            );
            let line = super::Strips::new([vec![[x, 10.0, -1.0], [x, 10.0, 2.4]]]).unwrap();
            let drawn = image(
                r,
                &Frame {
                    lines: &[magenta(&line)],
                    ..frame(&camera, &[], &[], None)
                },
            );
            // z from −1 to 2.4: rows 24 + 1.7 × 120/11 = 42.55 to 24 − 1.7 × 120/11 = 5.45.
            for y in 8..=40 {
                let got = pixel(&drawn, 45, y);
                assert!(within(got, MAGENTA, 8), "(45, {y}): {got:?}");
                for x in [42, 43] {
                    assert_eq!(pixel(&drawn, x, y), BLACK, "({x}, {y})");
                }
            }
        },
    );
}

#[test]
fn hostile_lines_are_refused_naming_lines_and_leave_the_renderer_drawing() {
    on_lavapipe(
        "render::frame_tests::hostile_lines_are_refused_naming_lines_and_leave_the_renderer_drawing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let refusal = |polylines: Vec<Vec<[f64; 3]>>| {
                let invalid = super::Strips::new(polylines).unwrap_err();
                (invalid.parameter, invalid.message)
            };
            let good = [0.0, 0.0, 0.0];
            for bad in [
                f64::NAN,
                f64::INFINITY,
                f64::NEG_INFINITY,
                1.000_000_1e12,
                -1.000_000_1e12,
            ] {
                for k in 0..3 {
                    let mut point = good;
                    point[k] = bad;
                    let (parameter, message) =
                        refusal(vec![vec![good, [1.0; 3]], vec![[1.0; 3], [2.0; 3], point]]);
                    assert_eq!(parameter, "lines");
                    assert!(message.starts_with("strip 1's point 2, "), "{message}");
                }
            }
            for short in [vec![], vec![good]] {
                let count = short.len();
                let (parameter, message) = refusal(vec![vec![good, [1.0; 3]], short]);
                assert_eq!(parameter, "lines");
                assert!(
                    message.starts_with(&format!("strip 1 has {count} point(s)")),
                    "{message}"
                );
            }
            // At the bound itself, made; none at all, made and empty.
            let edge = super::Strips::new([vec![[1e12, -1e12, 0.0], [0.0; 3]]]).unwrap();
            assert_eq!(
                (edge.len(), edge.points(), edge.bounds()),
                (1, 2, Some(([0.0, -1e12, 0.0], [1e12, 0.0, 0.0])))
            );
            let empty = super::Strips::new([]).unwrap();
            assert!(empty.is_empty() && empty.bounds().is_none());
            let line = line_at(0.0, ROW_26_Z);
            let good = frame(&camera, &[1], &[], None);
            for width in [0.0, -1.5, f32::NAN, f32::INFINITY, 64.5, -0.0] {
                let lines = [super::Lines {
                    width,
                    ..magenta(&line)
                }];
                assert_eq!(
                    refused(
                        r,
                        &Frame {
                            lines: &lines,
                            ..good
                        }
                    ),
                    "lines",
                    "{width}"
                );
            }
            for alpha in [0, 128, 254] {
                let lines = [super::Lines {
                    colour: [255, 0, 255, alpha],
                    ..magenta(&line)
                }];
                assert_eq!(
                    refused(
                        r,
                        &Frame {
                            lines: &lines,
                            ..good
                        }
                    ),
                    "lines",
                    "{alpha}"
                );
            }
            // A second set refused names its place.
            let lines = [
                magenta(&line),
                super::Lines {
                    width: 0.0,
                    ..magenta(&line)
                },
            ];
            let Err(RenderFailure::Invalid(invalid)) = r.prepare(&Frame {
                lines: &lines,
                ..good
            }) else {
                panic!("a width of 0 drawn");
            };
            assert!(
                invalid.message.starts_with("lines[1]'s width"),
                "{}",
                invalid.message
            );
            // The widest and a subnormal width are drawn, and an empty set draws nothing.
            for width in [64.0, f32::MIN_POSITIVE / 2.0] {
                r.prepare(&Frame {
                    lines: &[super::Lines {
                        width,
                        ..magenta(&line)
                    }],
                    ..good
                })
                .unwrap();
            }
            let without = image(r, &good);
            assert!(
                image(
                    r,
                    &Frame {
                        lines: &[magenta(&empty)],
                        ..good
                    }
                ) == without
            );
            // After every refusal the renderer still draws, and picks.
            assert_eq!(
                hit(r, &frame(&camera, &[1], &[], Some(ON_THE_FACE))),
                Some((1, Element::Face(3), [32, 26]))
            );
        },
    );
}

// ---- Concave edges drawn (N5's image) ----

/// A frame drawn and read back: its pixels row by row, `size` pixels.
struct Drawn {
    size: [u32; 2],
    pixels: Vec<[u8; 4]>,
}

impl Drawn {
    fn at(&self, [x, y]: [u32; 2]) -> [u8; 4] {
        self.pixels[usize::try_from(y * self.size[0] + x).unwrap()]
    }
}

/// `frame` drawn, with the render's light grey behind the bodies.
fn drawn_image(r: &mut Renderer, frame: &Frame<'_>) -> Drawn {
    let frame = Frame {
        background: Some(BACKGROUND),
        ..*frame
    };
    let commands = r.prepare(&frame).unwrap();
    Drawn {
        size: frame.size_px,
        pixels: composited_sized(r, Some(commands), wgpu::Color::TRANSPARENT, frame.size_px),
    }
}

/// The L of [`l_mesh`] with its inside corner filleted, radius 5, meshed as the app meshes it
/// for the view of [`l_camera`] (0.125 mm a pixel): at half a pixel, 0.0625 mm, a power of two
/// (`mesh_level`), the fillet's quarter circle in 5 steps of 18°, as OCCT's mesher and its
/// edges' sampling divide an arc of radius 5 (`ArcAngularStep`: 2 acos(1 − 0.0625/5) =
/// 18.13°, so ⌈90/18.13⌉ = 5 steps), its sagitta 5 (1 − cos 9°) = 0.0616 mm. Its profile in x
/// and z: (0, 0), (20, 0), (20, 5), (10, 5), the arc about (10, 10) to (5, 10), (5, 20),
/// (0, 20), along y from 0 to 20. Face1 the floor (z = 5, x 10 to 20), Face2 the fillet, its
/// points (10 − 5 sin φ, y, 10 − 5 cos φ) and its normals (sin φ, 0, cos φ), out of the solid,
/// Face3 the wall (x = 5, z 10 to 20), Face4 the upright's top, Face5 the back, Face6 the
/// bottom, Face7 the end, Face8 and Face9 the sides (y = 0 and 20). Edge1 the floor's tangent
/// line (10, y, 5), Edge2 the wall's (5, y, 10), Edge3 to Edge7 along y at (20, 5), (5, 20)
/// (the wall's top, convex), (0, 20), (0, 0), and (20, 0); then each side's outline.
fn filleted_l_mesh() -> Mesh {
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
    let arc: Vec<[f64; 2]> = (0..=5)
        .map(|i| {
            let phi = f64::from(i) * std::f64::consts::FRAC_PI_2 / 5.0;
            [10.0 - 5.0 * libm::sin(phi), 10.0 - 5.0 * libm::cos(phi)]
        })
        .collect();
    let normal_at = |i: u32| {
        let phi = f64::from(i) * std::f64::consts::FRAC_PI_2 / 5.0;
        [libm::sin(phi) as f32, 0.0, libm::cos(phi) as f32]
    };
    /// A triangle's corners, and their normals.
    type Triangle = ([[f64; 3]; 3], [[f32; 3]; 3]);
    let mut face = |triangles: Vec<Triangle>| {
        for (corners, normals) in &triangles {
            let base = u32::try_from(mesh.positions.len()).unwrap();
            mesh.positions.extend(corners);
            mesh.normals.extend(normals);
            mesh.triangles.push([base, base + 1, base + 2]);
        }
        mesh.face_triangles
            .push(u32::try_from(triangles.len()).unwrap());
    };
    let quad = |c: [[f64; 3]; 4], n: [f32; 3]| {
        vec![([c[0], c[1], c[2]], [n; 3]), ([c[0], c[2], c[3]], [n; 3])]
    };
    let along =
        |[x, z]: [f64; 2], [u, w]: [f64; 2]| [[x, 0.0, z], [u, 0.0, w], [u, 20.0, w], [x, 20.0, z]];
    face(quad(along([10.0, 5.0], [20.0, 5.0]), [0.0, 0.0, 1.0]));
    face(
        (0..5u32)
            .flat_map(|i| {
                let k = usize::try_from(i).unwrap();
                let c = along(arc[k], arc[k + 1]);
                let (a, b) = (normal_at(i), normal_at(i + 1));
                [
                    ([c[0], c[1], c[2]], [a, b, b]),
                    ([c[0], c[2], c[3]], [a, b, a]),
                ]
            })
            .collect(),
    );
    face(quad(along([5.0, 10.0], [5.0, 20.0]), [1.0, 0.0, 0.0]));
    face(quad(along([0.0, 20.0], [5.0, 20.0]), [0.0, 0.0, 1.0]));
    face(quad(along([0.0, 0.0], [0.0, 20.0]), [-1.0, 0.0, 0.0]));
    face(quad(along([0.0, 0.0], [20.0, 0.0]), [0.0, 0.0, -1.0]));
    face(quad(along([20.0, 0.0], [20.0, 5.0]), [1.0, 0.0, 0.0]));
    for (y, n) in [(0.0, -1.0f32), (20.0, 1.0)] {
        let at = |[x, z]: [f64; 2]| [x, y, z];
        let normal = [0.0, n, 0.0];
        let mut side = quad(
            [[0.0, 0.0], [5.0, 0.0], [5.0, 20.0], [0.0, 20.0]].map(at),
            normal,
        );
        side.extend(quad(
            [[5.0, 0.0], [20.0, 0.0], [20.0, 5.0], [5.0, 5.0]].map(at),
            normal,
        ));
        side.extend((0..5).map(|i| ([at([5.0, 5.0]), at(arc[i]), at(arc[i + 1])], [normal; 3])));
        face(side);
    }
    let mut edges: Vec<Vec<[f64; 3]>> = vec![
        vec![[10.0, 0.0, 5.0], [10.0, 20.0, 5.0]],
        vec![[5.0, 0.0, 10.0], [5.0, 20.0, 10.0]],
    ];
    for [x, z] in [
        [20.0, 5.0],
        [5.0, 20.0],
        [0.0, 20.0],
        [0.0, 0.0],
        [20.0, 0.0],
    ] {
        edges.push(vec![[x, 0.0, z], [x, 20.0, z]]);
    }
    for y in [0.0, 20.0] {
        let at = |[x, z]: [f64; 2]| [x, y, z];
        for (a, b) in [
            ([0.0, 0.0], [20.0, 0.0]),
            ([20.0, 0.0], [20.0, 5.0]),
            ([20.0, 5.0], [10.0, 5.0]),
            ([5.0, 10.0], [5.0, 20.0]),
            ([5.0, 20.0], [0.0, 20.0]),
            ([0.0, 20.0], [0.0, 0.0]),
        ] {
            edges.push(vec![at(a), at(b)]);
        }
        edges.push(arc.iter().copied().map(at).collect());
    }
    for points in edges {
        mesh.edge_point_counts
            .push(u32::try_from(points.len()).unwrap());
        mesh.edge_points.extend(points);
    }
    for y in [0.0, 20.0] {
        for [x, z] in [
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 5.0],
            [10.0, 5.0],
            [5.0, 10.0],
            [5.0, 20.0],
            [0.0, 20.0],
        ] {
            mesh.vertex_points.push([x, y, z]);
        }
    }
    mesh
}

/// A line of the image from `a` to `b`, in pixels, worked by hand: each pixel's centre
/// measured along it from `a`, and off it, positive to the right of its direction as the image
/// is drawn (x right, y down), so for a line going right and 30° down, `s` = 0.866 (y − a.y)
/// − 0.5 (x − a.x), below it positive (the study's `s`).
struct ImageLine {
    a: [f64; 2],
    along: [f64; 2],
    length: f64,
}

/// Which side of an [`ImageLine`] a pixel's centre is: the right, `s` ≥ 0, then the left.
const SIDES: [&str; 2] = ["right", "left"];

impl ImageLine {
    fn new(a: [f64; 2], b: [f64; 2]) -> ImageLine {
        let d = [b[0] - a[0], b[1] - a[1]];
        let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
        ImageLine {
            a,
            along: [d[0] / length, d[1] / length],
            length,
        }
    }

    /// The pixel `pixel`'s centre: how far along the line from `a`, and how far off it.
    fn measure(&self, [x, y]: [u32; 2]) -> (f64, f64) {
        let c = [
            f64::from(x) + 0.5 - self.a[0],
            f64::from(y) + 0.5 - self.a[1],
        ];
        let [u, v] = self.along;
        (u * c[0] + v * c[1], u * c[1] - v * c[0])
    }

    /// The pixel whose centre is nearest the point `t` along the line and `s` off it.
    fn pixel_at(&self, t: f64, s: f64) -> [u32; 2] {
        let [u, v] = self.along;
        let p = [self.a[0] + t * u - s * v, self.a[1] + t * v + s * u];
        p.map(|c| u32::try_from(c.floor() as i64).unwrap())
    }

    /// Of the pixels of `image` whose centres lie from `margin` along the line to `margin`
    /// before its end, and `low` to `high` off it (`low` ≤ |s| < `high`), how many on each
    /// side ([`SIDES`]) `test` takes of all there, as (taken, of): `test` given the pixel's
    /// colour and its face's, the pixel `face` off the line on its side, across from it.
    fn tally(
        &self,
        image: &Drawn,
        margin: f64,
        (low, high): (f64, f64),
        face: f64,
        test: impl Fn([u8; 4], [u8; 4]) -> bool,
    ) -> [(usize, usize); 2] {
        let mut sides = [(0, 0); 2];
        let reach = high.max(face) + 2.0;
        let end = [
            self.a[0] + self.length * self.along[0],
            self.a[1] + self.length * self.along[1],
        ];
        let from = |k: usize| (self.a[k].min(end[k]) - reach).floor().max(0.0) as u32;
        let to = |k: usize| {
            let most = f64::from(image.size[k] - 1);
            (self.a[k].max(end[k]) + reach).ceil().min(most) as u32
        };
        for y in from(1)..=to(1) {
            for x in from(0)..=to(0) {
                let (t, s) = self.measure([x, y]);
                if t < margin || t > self.length - margin || s.abs() < low || s.abs() >= high {
                    continue;
                }
                let side = usize::from(s < 0.0);
                let beside = self.pixel_at(t, face.copysign(s));
                sides[side].1 += 1;
                if test(image.at([x, y]), image.at(beside)) {
                    sides[side].0 += 1;
                }
            }
        }
        sides
    }
}

/// Whether a pixel is the edges' colour, to 3 of 255 a channel.
fn edge_coloured(pixel: [u8; 4], _: [u8; 4]) -> bool {
    (0..3).all(|k| pixel[k].abs_diff(EDGE[k]) <= 3)
}

/// Whether a pixel is darker than its face by 15 of 255 or more over its channels: blended
/// with the edges' colour.
fn darker(pixel: [u8; 4], face: [u8; 4]) -> bool {
    let sum = |p: [u8; 4]| p[..3].iter().map(|&c| i32::from(c)).sum::<i32>();
    sum(face) - sum(pixel) >= 15
}

/// Whether a pixel is its face's colour, to 1 of 255 a channel: nothing drawn over it.
fn its_face(pixel: [u8; 4], face: [u8; 4]) -> bool {
    (0..3).all(|k| pixel[k].abs_diff(face[k]) <= 1)
}

/// Whether every pixel of a tally was taken, on both sides, of at least `least` a side.
fn all_of(sides: [(usize, usize); 2], least: usize) -> bool {
    sides.iter().all(|&(taken, of)| taken == of && of >= least)
}

/// An edge's stroke along `line`'s middle (`margin` from its ends) in `image`, `half` pixels
/// from its line to each side (a point at `half` pixels a point), on each side: the pixels it
/// covers whole (their centres within `half` − 1/2 of the line, so each of their four samples
/// under it, at its whole coverage), which are the edges' colour where it's drawn; and those it
/// covers in part (from there to `half` − 0.1), which are darker than the face beside them
/// (`half` + 2.5 off the line) where it's drawn, and their face's colour where it's hidden.
fn stroke_of(image: &Drawn, line: &ImageLine, half: f64, margin: f64) -> [[(usize, usize); 2]; 2] {
    let face = half + 2.5;
    [
        line.tally(image, margin, (0.0, half - 0.5), face, edge_coloured),
        line.tally(image, margin, (half - 0.5, half - 0.1), face, darker),
    ]
}

/// Of the pixels of `image` whose centres lie from `margin` along `line` to `margin` before its
/// end, and `low` to `high` off it (`low` ≤ |s| < `high`), each one's `s` and its colour.
fn band(
    image: &Drawn,
    line: &ImageLine,
    margin: f64,
    (low, high): (f64, f64),
) -> Vec<(f64, [u8; 4])> {
    let mut found = Vec::new();
    for y in 0..image.size[1] {
        for x in 0..image.size[0] {
            let (t, s) = line.measure([x, y]);
            if t < margin || t > line.length - margin || s.abs() < low || s.abs() >= high {
                continue;
            }
            found.push((s, image.at([x, y])));
        }
    }
    found
}

/// A frame of the L or another body, body 1, `size` pixels at `pixels_per_point`, picking
/// nothing.
fn plain_frame<'a>(camera: &'a Camera, size: [u32; 2], pixels_per_point: f32) -> Frame<'a> {
    Frame {
        size_px: size,
        pixels_per_point,
        ..frame(camera, &[1], &[], None)
    }
}

/// Where `p` lands in [`l_camera`]'s view, `k` pixels a millimetre about the image's centre
/// `c` (8 and 400 at 800 rows, 16 and 800 at 1600), by [`corner_view`].
fn l_view(p: [f64; 3], k: f64, c: f64) -> [f64; 2] {
    corner_view(p, [5.0, 10.0, 5.0], k, c)
}

#[test]
fn a_concave_edges_stroke_is_drawn_whole_along_its_line_as_a_convex_ones_is() {
    on_lavapipe(
        "render::frame_tests::a_concave_edges_stroke_is_drawn_whole_along_its_line_as_a_convex_ones_is",
        |r| {
            // The L from its corner, as the study sees it: 100 mm over 800 rows, a pixel a
            // point. Edge1, the concave (5, y, 5), from (343.431, 367.340) to (456.569,
            // 432.660); Edge4, the convex wall top (5, y, 20), 240/√6 = 97.98 rows above it.
            // Across either line each face beside it rises toward the eye by √1.5 = 1.2247
            // pixels of depth a pixel (0.153 mm, a pixel being 0.125 mm), at Edge1 away from
            // it, at Edge4 toward it. Its stroke reaches 0.99 of a point (a pixel) either side,
            // and its four samples a pixel 0.3 off its centre across it: so the faces hid
            // Edge1's every sample (more than 2e-5 pixels off), and now, moved toward the eye
            // by √2 + 1/2 = 1.91 pixels of depth, past their 1.2247 × 0.99 = 1.21, it's drawn
            // whole: each pixel whose centre is within half a pixel of its line the edges'
            // colour, and each from there to 0.9 off darker than its face, as Edge4's always
            // were. Along each line's middle, 8 pixels in from its ends, where the profile's
            // edges meet it: about 57 pixels a side whole, and 46 in part.
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = l_camera();
            let ends = |z: f64| {
                [
                    l_view([5.0, 0.0, z], 8.0, 400.0),
                    l_view([5.0, 20.0, z], 8.0, 400.0),
                ]
            };
            let r2 = 2f64.sqrt();
            let r6 = 6f64.sqrt();
            assert_pixel(
                Some(ends(5.0)[0]),
                [400.0 - 80.0 / r2, 400.0 - 80.0 / r6],
                1e-9,
            );
            assert_pixel(
                Some(ends(20.0)[1]),
                [400.0 + 80.0 / r2, 400.0 - 160.0 / r6],
                1e-9,
            );
            for z in [5.0, 20.0] {
                for (k, y) in [0.0, 20.0].into_iter().enumerate() {
                    assert_pixel(project(&camera, SQUARE, [5.0, y, z]), ends(z)[k], 1e-4);
                }
            }
            let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
            for (what, z) in [("Edge4, convex", 20.0), ("Edge1, concave", 5.0)] {
                let [a, b] = ends(z);
                let [whole, part] = stroke_of(&image, &ImageLine::new(a, b), 1.0, 8.0);
                assert!(all_of(whole, 50), "{what}: whole {whole:?} of {SIDES:?}");
                assert!(all_of(part, 40), "{what}: in part {part:?} of {SIDES:?}");
            }
            // Those bands hold for a reach over about 0.98 pixels, though a whole stroke over
            // these faces needs 1.21 (the test skeptic, #439). So each of Edge1's pixels 0.5 to
            // 0.9 off its line, partly under its stroke, is compared with Edge4's as far off on
            // the side whose face faces the same way: the floor below Edge1 and the top above
            // Edge4 face +z, and the wall is above Edge1 and below Edge4. Both lines run 30°
            // below the row, and a pixel's four samples are symmetric about its centre, so a
            // pixel and its mirror have their samples as far off their lines, and a stroke
            // drawn whole over both covers them alike. On lavapipe 89 of the 90 agree to a
            // level a channel; one, (433, 418), 0.73 off on the wall's side, is 10 lighter,
            // as light with a reach of 1.1 pixels as with 1.91, so no face took its sample
            // (its faces there are 1.26 pixels of depth nearer at most): each side may have
            // one. A reach of 1.1 pixels (√2 taken as 0.6) left 14 and 13 a side lighter by 9
            // to 57 levels.
            let [a, b] = ends(5.0);
            let concave = band(&image, &ImageLine::new(a, b), 8.0, (0.5, 0.9));
            let [a, b] = ends(20.0);
            let convex = band(&image, &ImageLine::new(a, b), 8.0, (0.5, 0.9));
            let mut pairs = [0usize; 2];
            let mut apart = [0usize; 2];
            for &(s, pixel) in &concave {
                let &(mirror, its) = convex
                    .iter()
                    .min_by(|m, n| (m.0 + s).abs().total_cmp(&(n.0 + s).abs()))
                    .unwrap();
                assert!((mirror + s).abs() < 0.01, "{s} against {mirror}");
                let side = usize::from(s < 0.0);
                pairs[side] += 1;
                if (0..3).any(|k| pixel[k].abs_diff(its[k]) > 2) {
                    apart[side] += 1;
                }
            }
            assert!(
                pairs.iter().all(|&n| n >= 40) && apart.iter().all(|&n| n <= 1),
                "{apart:?} of {pairs:?} apart from Edge4's, of {SIDES:?}"
            );
        },
    );
}

#[test]
fn a_fillets_tangent_edges_are_drawn_whole_along_their_lines_over_the_flat_faces_beside_them() {
    on_lavapipe(
        "render::frame_tests::a_fillets_tangent_edges_are_drawn_whole_along_their_lines_over_the_flat_faces_beside_them",
        |r| {
            // The L with its inside corner filleted, meshed as the view meshes it (half a
            // pixel, the arc in 18° steps), from its corner as above. Edge1, the floor's
            // tangent line (10, y, 5), from (315.147, 383.670) to (428.284, 448.990), the floor
            // to its right (s > 0); Edge2, the wall's (5, y, 10), from (343.431, 334.680) to
            // (456.569, 400.000), the wall to its left. Each flat face rises toward the eye
            // away from its line by √1.5 pixels of depth a pixel, as the L's do, and hid the
            // half of its stroke on its side; the fillet beside each recedes from it (its
            // first chord, turned 9° from the flat face toward the eye, by cos 9° − sin 9° =
            // 0.83 of the flat face's slope), so the other half was drawn. On the dark wall
            // the half left drew a rim a point wide 10 of 255 darker than the wall, which on
            // a display reads as no edge at all. Now each is drawn whole, as the convex Edge4
            // (5, y, 20) is.
            r.upload(1, &Arc::new(filleted_l_mesh())).unwrap();
            let camera = l_camera();
            let r2 = 2f64.sqrt();
            let r6 = 6f64.sqrt();
            let lines = [
                (
                    "Edge4, convex",
                    [5.0, 0.0, 20.0],
                    [400.0 - 80.0 / r2, 400.0 - 320.0 / r6],
                ),
                (
                    "Edge1, the floor's",
                    [10.0, 0.0, 5.0],
                    [400.0 - 120.0 / r2, 400.0 - 40.0 / r6],
                ),
                (
                    "Edge2, the wall's",
                    [5.0, 0.0, 10.0],
                    [400.0 - 80.0 / r2, 400.0 - 160.0 / r6],
                ),
            ];
            let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
            for (what, start, want) in lines {
                let end = [start[0], 20.0, start[2]];
                let a = l_view(start, 8.0, 400.0);
                assert_pixel(Some(a), want, 1e-9);
                assert_pixel(project(&camera, SQUARE, start), a, 1e-4);
                let b = l_view(end, 8.0, 400.0);
                assert_pixel(project(&camera, SQUARE, end), b, 1e-4);
                let [whole, part] = stroke_of(&image, &ImageLine::new(a, b), 1.0, 8.0);
                assert!(all_of(whole, 50), "{what}: whole {whole:?} of {SIDES:?}");
                assert!(all_of(part, 40), "{what}: in part {part:?} of {SIDES:?}");
            }
        },
    );
}

#[test]
fn at_two_pixels_a_point_a_concave_edge_is_drawn_within_an_edges_picking_reach_and_no_farther() {
    on_lavapipe(
        "render::frame_tests::at_two_pixels_a_point_a_concave_edge_is_drawn_within_an_edges_picking_reach_and_no_farther",
        |r| {
            // The L at 1600 by 1600 at two pixels a point (a display's), the same view: 16
            // pixels a millimetre about (800, 800). The stroke reaches 1.98 pixels either side
            // of its line, and the least reach that draws it whole over faces rising √2 a pixel
            // would be 2√2 + 1/2 = 3.33 pixels of depth; it's held to an edge's picking reach,
            // 2 (0.125 mm here). So over the L's faces, rising 1.2247 a pixel, Edge1 is drawn to
            // 2/1.2247 = 1.63 pixels off its line: each pixel within 1.2 of it whole (its
            // samples within 1.5, the faces there at most 1.84 pixels of depth nearer the eye)
            // the edges' colour; each 2 to 2.4 off (its samples under the stroke 1.70 off or
            // more, the faces 2.08 nearer or more) its face's, as it was. At 3.33 pixels they'd
            // be darker. The convex Edge4 is drawn whole, 1.5 pixels either side, as before.
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = l_camera();
            let size = [1600, 1600];
            let ends = |z: f64| {
                [
                    l_view([5.0, 0.0, z], 16.0, 800.0),
                    l_view([5.0, 20.0, z], 16.0, 800.0),
                ]
            };
            for z in [5.0, 20.0] {
                for (k, y) in [0.0, 20.0].into_iter().enumerate() {
                    assert_pixel(project(&camera, size, [5.0, y, z]), ends(z)[k], 1e-4);
                }
            }
            let image = drawn_image(r, &plain_frame(&camera, size, 2.0));
            let [a, b] = ends(5.0);
            let concave = ImageLine::new(a, b);
            let near = concave.tally(&image, 16.0, (0.0, 1.2), 4.5, edge_coloured);
            assert!(all_of(near, 200), "Edge1 within 1.2: {near:?} of {SIDES:?}");
            let past = concave.tally(&image, 16.0, (2.0, 2.4), 4.5, its_face);
            assert!(
                all_of(past, 80),
                "Edge1 2 to 2.4 off: {past:?} of {SIDES:?}"
            );
            let [a, b] = ends(20.0);
            let [whole, part] = stroke_of(&image, &ImageLine::new(a, b), 2.0, 16.0);
            assert!(all_of(whole, 250), "Edge4: whole {whole:?} of {SIDES:?}");
            assert!(all_of(part, 80), "Edge4: in part {part:?} of {SIDES:?}");
        },
    );
}

/// Where `p` lands seen from (1, 1, 1) about `target`, z up, orthographic: `k` pixels a
/// millimetre about the image's centre `c`, by the view's axes worked by hand, right
/// (−1, 1, 0)/√2 and up (−1, −1, 2)/√6: at (c + k right · d, c − k up · d), d = p − target.
fn corner_view(p: [f64; 3], target: [f64; 3], k: f64, c: f64) -> [f64; 2] {
    let d = [p[0] - target[0], p[1] - target[1], p[2] - target[2]];
    [
        c + k * (d[1] - d[0]) / 2f64.sqrt(),
        c - k * (2.0 * d[2] - d[0] - d[1]) / 6f64.sqrt(),
    ]
}

#[test]
fn edges_behind_the_body_stay_hidden_in_the_drawn_image() {
    on_lavapipe(
        "render::frame_tests::edges_behind_the_body_stay_hidden_in_the_drawn_image",
        |r| {
            // Each pixel within 1.5 of a hidden edge's line, along its middle, is its face's
            // colour, the face 4 or 5 pixels off it: nothing of the edge drawn there.
            // The L's Edge2, (0, 0, z), its back corner: down column 400 − 40/√2 = 371.716,
            // from row 400 − 360/√6 = 253.031 (z = 20) to 400 − 40/√6 = 383.670 (z = 0). Seen
            // through the upright's top (Face3, z = 20) for z from 20 to 15, row 285.70, where
            // the wall's top edge (Edge4) crosses it, then through the wall (Face2, x = 5),
            // 5√3 = 8.66 mm nearer the eye; the top is (20 − z)√3 nearer, under the drawn
            // reach (0.239 mm) only within 0.14 mm of z = 20, 0.9 pixels of its end. Each
            // stretch 5 pixels in from where an edge meets it.
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = l_camera();
            let at = |z: f64| l_view([0.0, 0.0, z], 8.0, 400.0);
            let r2 = 2f64.sqrt();
            let r6 = 6f64.sqrt();
            assert_pixel(
                Some(at(20.0)),
                [400.0 - 40.0 / r2, 400.0 - 360.0 / r6],
                1e-9,
            );
            assert_pixel(
                Some(at(15.0)),
                [400.0 - 40.0 / r2, 400.0 - 280.0 / r6],
                1e-9,
            );
            assert_pixel(Some(at(0.0)), [400.0 - 40.0 / r2, 400.0 - 40.0 / r6], 1e-9);
            for z in [20.0, 15.0, 0.0] {
                assert_pixel(project(&camera, SQUARE, [0.0, 0.0, z]), at(z), 1e-4);
            }
            let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
            for (what, from, to) in [
                ("through the top", 20.0, 15.0),
                ("through the wall", 15.0, 0.0),
            ] {
                let line = ImageLine::new(at(from), at(to));
                let near = line.tally(&image, 5.0, (0.0, 1.5), 4.0, its_face);
                assert!(all_of(near, 20), "Edge2 {what}: {near:?} of {SIDES:?}");
            }
            // The 20 mm cube from its corner, 80 mm over 800 rows (10 pixels a millimetre):
            // its corner (0, 0, 0) is behind the nearest, (20, 20, 20), at the image's centre,
            // and its three edges each lie along a seen face's diagonal, from 20√3 = 34.6 mm
            // behind it at the centre to the face's far corner, on the outline, so 0.212 mm a
            // pixel: under the reach (0.191 mm here) only within 0.9 pixels of that corner. Each
            // 16 pixels in from its ends, where the face is 9 pixels or more either side of it.
            r.upload(1, &Arc::new(box_mesh([0.0; 3], [20.0; 3])))
                .unwrap();
            let camera = corner_camera(Projection::Orthographic { view_height: 80.0 });
            let at = |p: [f64; 3]| corner_view(p, [10.0; 3], 10.0, 400.0);
            let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
            for (what, far, want) in [
                (
                    "Edge1, along x",
                    [20.0, 0.0, 0.0],
                    [400.0 - 200.0 / r2, 400.0 + 200.0 / r6],
                ),
                (
                    "Edge9, along y",
                    [0.0, 20.0, 0.0],
                    [400.0 + 200.0 / r2, 400.0 + 200.0 / r6],
                ),
                (
                    "Edge4, along z",
                    [0.0, 0.0, 20.0],
                    [400.0, 400.0 - 400.0 / r6],
                ),
            ] {
                assert_pixel(Some(at(far)), want, 1e-9);
                assert_pixel(project(&camera, SQUARE, far), want, 1e-4);
                assert_pixel(project(&camera, SQUARE, [0.0; 3]), [400.0, 400.0], 1e-4);
                let line = ImageLine::new([400.0, 400.0], want);
                let near = line.tally(&image, 16.0, (0.0, 1.5), 5.0, its_face);
                assert!(all_of(near, 100), "{what}: {near:?} of {SIDES:?}");
            }
        },
    );
}

#[test]
fn an_edge_behind_a_thin_wall_is_drawn_through_it_only_within_its_drawn_reach() {
    on_lavapipe(
        "render::frame_tests::an_edge_behind_a_thin_wall_is_drawn_through_it_only_within_its_drawn_reach",
        |r| {
            // The plate from above, its Edge13 along row 400.0 (`above_the_wall`): its stroke
            // covers rows 399 and 400 whole (their centres half a pixel off it, each sample
            // within 0.875), checked from column 340 to 460, the plate's top 10 rows off the
            // face beside them. Orthographic, the drawn reach is √2 + 1/2 = 1.9142 pixels of
            // depth, 0.2393 mm: the edge 1 mm (zb 0) or 0.3 mm (0.7) behind the plate's top is
            // hidden, as at 0.245 (0.755), past the reach though within an edge's picking reach
            // (0.25); at 0.23 (0.77), past √2 pixels (0.177) but within the half pixel more,
            // and at 0.2 (0.8), it's drawn. In perspective, the reach is 1.9142/800 of the
            // depth along the line to the eye, so it's at (101 − zb) × 0.9976072 against the
            // top at 100: 100.758, 100.060, and 100.005, behind; 99.990 and 99.960, in front.
            for perspective in [false, true] {
                let camera = above_the_wall(perspective);
                for (zb, drawn) in [
                    (0.0, false),
                    (0.7, false),
                    (0.755, false),
                    (0.77, true),
                    (0.8, true),
                ] {
                    let end = if perspective {
                        400.0 - 10.0 * 800.0 / (101.0 - zb)
                    } else {
                        320.0
                    };
                    assert_pixel(
                        project(&camera, SQUARE, [10.0, 20.0, zb]),
                        [end, 400.0],
                        1e-4,
                    );
                    r.upload(1, &thin_wall(zb)).unwrap();
                    let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
                    let line = ImageLine::new([330.0, 400.0], [470.0, 400.0]);
                    let test = if drawn { edge_coloured } else { its_face };
                    let got = line.tally(&image, 10.0, (0.0, 0.6), 10.0, test);
                    let what = format!("zb {zb}, perspective {perspective}, drawn {drawn}");
                    assert!(all_of(got, 120), "{what}: {got:?} of {SIDES:?}");
                }
            }
        },
    );
}

/// The L's Edge5, (0, y, 20), the upright top's back edge, from (400 − 40/√2, 400 − 360/√6) =
/// (371.716, 253.031) to (400 + 120/√2, 400 − 200/√6) = (484.853, 318.350), checked by hand
/// against `project`: the back (x = 0) faces away, so to its left is nothing, the render's
/// grey, and to its right the upright's top, which rises toward the eye away from it by √1.5
/// pixels of depth a pixel, as a concave edge's faces do.
fn silhouette(camera: &Camera) -> ImageLine {
    let at = |y: f64| l_view([0.0, y, 20.0], 8.0, 400.0);
    let r2 = 2f64.sqrt();
    let r6 = 6f64.sqrt();
    assert_pixel(Some(at(0.0)), [400.0 - 40.0 / r2, 400.0 - 360.0 / r6], 1e-9);
    assert_pixel(
        Some(at(20.0)),
        [400.0 + 120.0 / r2, 400.0 - 200.0 / r6],
        1e-9,
    );
    for y in [0.0, 20.0] {
        assert_pixel(project(camera, SQUARE, [0.0, y, 20.0]), at(y), 1e-4);
    }
    ImageLine::new(at(0.0), at(20.0))
}

/// Whether a pixel is the render's grey, to 1 of 255 a channel.
fn grey(pixel: [u8; 4], _: [u8; 4]) -> bool {
    (0..3).all(|k| pixel[k].abs_diff(BACKGROUND[k]) <= 1)
}

/// The plate's top's edge at x = 0 in perspective from above (`above_the_wall`), with the
/// plate drawn: 100 mm from the eye, down column 400 − 20 × 800/100 = 240.0, the side below it
/// facing away, the line from row 250 to 550 (y from 37.5 to 0). Its stroke covers columns 239
/// and 240 whole, their centres half a pixel off it, outside and over the top. Its bottom edge
/// at x = 0, 101 mm from the eye, lies down column 400 − 20 × 800/101 = 241.584, 1 mm behind the
/// top, past the drawn reach there (1.9142/800 × 101 = 0.242 mm), so hidden: columns 241 and 242
/// would be under its stroke (their centres 0.08 and 0.92 off it).
fn the_plates_top_edge(r: &mut Renderer) -> (Drawn, ImageLine) {
    r.upload(1, &thin_wall(0.0)).unwrap();
    let camera = above_the_wall(true);
    assert_pixel(
        project(&camera, SQUARE, [0.0, 20.0, 1.0]),
        [240.0, 400.0],
        1e-4,
    );
    assert_pixel(
        project(&camera, SQUARE, [0.0, 20.0, 0.0]),
        [400.0 - 20.0 * 800.0 / 101.0, 400.0],
        1e-4,
    );
    let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
    (image, ImageLine::new([240.0, 250.0], [240.0, 550.0]))
}

#[test]
fn a_silhouette_is_drawn_whole_over_the_face_it_bounds() {
    on_lavapipe(
        "render::frame_tests::a_silhouette_is_drawn_whole_over_the_face_it_bounds",
        |r| {
            // The L's Edge5 ([`silhouette`]): the top beside it hid the right half of its
            // stroke, as at a concave edge; now its stroke is drawn whole on both sides.
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = l_camera();
            let line = silhouette(&camera);
            let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
            let [whole, part] = stroke_of(&image, &line, 1.0, 8.0);
            assert!(all_of(whole, 50), "Edge5: whole {whole:?} of {SIDES:?}");
            assert!(all_of(part, 40), "Edge5: in part {part:?} of {SIDES:?}");
            // The plate's top's edge in perspective ([`the_plates_top_edge`]): the top is square
            // to the view's axis, but off it the stroke is spanned square to its own ray, 11.3°
            // off the axis at column 240 (tan = 20/100), so it sank into the top across its
            // inner half, by 0.2 pixels of depth a pixel, and column 240 showed the top. Now
            // both columns are drawn, darker than the grey 10 to the left and the top 10 to the
            // right, rows 260 to 539.
            let (image, line) = the_plates_top_edge(r);
            let on = line.tally(&image, 10.0, (0.0, 0.6), 10.0, darker);
            assert!(all_of(on, 280), "the top's edge: {on:?} of {SIDES:?}");
        },
    );
}

#[test]
fn a_silhouettes_stroke_over_nothing_and_whats_past_it_are_drawn_as_they_were() {
    on_lavapipe(
        "render::frame_tests::a_silhouettes_stroke_over_nothing_and_whats_past_it_are_drawn_as_they_were",
        |r| {
            // What moving a stroke along its line to the eye must leave as it was, since it keeps
            // its pixels. The L's Edge5 ([`silhouette`]): to its left, from 0.35 pixels off (its
            // samples 0.05 off or more, over nothing) to 0.9, darker than the grey; from 1.45 (its
            // samples past 0.99, the stroke's reach) to 6, the grey; and from 1.45 to 3 off on
            // either side, the colour 6 off, the grey or the top.
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = l_camera();
            let line = silhouette(&camera);
            let image = drawn_image(r, &plain_frame(&camera, SQUARE, 1.0));
            let [_, rim] = line.tally(&image, 8.0, (0.35, 0.9), 6.0, darker);
            assert!(rim.0 == rim.1 && rim.1 >= 50, "Edge5's left rim: {rim:?}");
            let [_, left] = line.tally(&image, 8.0, (1.45, 6.0), 6.0, grey);
            assert!(left.0 == left.1 && left.1 >= 400, "left of Edge5: {left:?}");
            let past = line.tally(&image, 8.0, (1.45, 3.0), 6.0, its_face);
            assert!(
                all_of(past, 150),
                "past Edge5's stroke: {past:?} of {SIDES:?}"
            );
            // The plate's top's edge in perspective ([`the_plates_top_edge`]), rows 260 to 539:
            // column 239, outside, darker than the grey 10 to its left; 237 and 238, the grey;
            // 241 and 242, past the stroke and over the hidden bottom edge, the top's colour.
            let (image, line) = the_plates_top_edge(r);
            let [outside, _] = line.tally(&image, 10.0, (0.0, 0.6), 10.0, darker);
            assert!(
                outside.0 == outside.1 && outside.1 == 280,
                "column 239: {outside:?}"
            );
            let past = line.tally(&image, 10.0, (1.4, 2.6), 10.0, its_face);
            assert!(all_of(past, 560), "past it: {past:?} of {SIDES:?}");
            for row in [260, 400, 539] {
                let far = image.at([230, row]);
                assert!(grey(far, far), "({row}): {far:?}");
            }
        },
    );
}

// ---- Hostile drawn edges (N5's image) ----

#[test]
fn an_edge_picks_point_is_on_its_line_whether_edges_are_picked_or_not_at_any_pixels_a_point() {
    on_lavapipe(
        "render::frame_tests::an_edge_picks_point_is_on_its_line_whether_edges_are_picked_or_not_at_any_pixels_a_point",
        |r| {
            // Found by /break-it: with edges not picked (the selection's edge filter off), an
            // edge pick's point was its drawn strip's taken back from the eye by an edge's
            // picking reach, 2 pixels of depth, though the strip was moved by its own, √2 h +
            // 1/2 (h the stroke's half width in pixels): left behind its line by the
            // difference, 2 − 1.9142 = 0.0858 pixels of depth at a pixel a point (0.0107 mm,
            // z 0.9893) and 2 − (0.75 √2 + 1/2) = 0.4393 at 3/4 (0.0549 mm, z 0.9451), past
            // the 0.09 the notes gave.
            //
            // Edge13 on the plate's top (zb = 1), from above ([`above_the_wall`]), along row
            // 400.0: the pixel (400, 400), its centre half a pixel right of the line's point
            // (20, 20, 1) and half a pixel below it, is under its stroke at 3/4, 1, and 2
            // pixels a point (half widths of 0.75 pixels and more), the strip drawn over the
            // top (moved toward the eye). Its point there, by hand: (20 + 0.0625, 20 − 0.0625,
            // 1), half a pixel each way being 0.0625 mm orthographic (0.125 mm a pixel) and in
            // perspective at the top's depth, 100 mm, where a pixel is 100/800 mm. With edges
            // picked, it's the picking strip's, taken back by its reach; with them not, the
            // drawn strip's, taken back by that strip's; at 2 pixels a point the two are one.
            r.upload(1, &thin_wall(1.0)).unwrap();
            for perspective in [false, true] {
                let camera = above_the_wall(perspective);
                assert_pixel(
                    project(&camera, SQUARE, [20.0, 20.0, 1.0]),
                    [400.0, 400.0],
                    1e-4,
                );
                for pixels_per_point in [0.75, 1.0, 2.0] {
                    for edges in [true, false] {
                        let what = format!(
                            "perspective {perspective}, {pixels_per_point} pixels a point, edges {edges}"
                        );
                        let asking = Frame {
                            size_px: SQUARE,
                            pixels_per_point,
                            edges,
                            ..frame(&camera, &[1], &[], Some(rect([400, 400], [1, 1])))
                        };
                        let hit = pick(r, &asking).hit.unwrap();
                        assert_eq!(
                            (hit.body, hit.hit, hit.pixel),
                            (1, Element::Edge(13), [400, 400]),
                            "{what}"
                        );
                        let got = hit.point.unwrap();
                        let want = [20.0625, 19.9375, 1.0];
                        assert!(
                            (0..3).all(|k| (got[k] - want[k]).abs() < 1e-3),
                            "{what}: {got:?} against {want:?}"
                        );
                    }
                }
            }
        },
    );
}

#[test]
fn a_drawn_edge_behind_a_thin_wall_follows_the_reach_of_the_frames_pixels_a_point() {
    on_lavapipe(
        "render::frame_tests::a_drawn_edge_behind_a_thin_wall_follows_the_reach_of_the_frames_pixels_a_point",
        |r| {
            // The plate from above, its Edge13 along row 400.0 (`above_the_wall`): rows 399 and
            // 400, their centres half a pixel off it, under its stroke at half a pixel a point
            // (its half width half a pixel: half of each) and at two (two pixels: all of each),
            // from column 340 to 460, the plate's top 10 rows off beside them. The drawn reach
            // is √2 h + 1/2 pixels of depth, h the stroke's half width in pixels, at most an
            // edge's picking reach, 2: at half a pixel a point √2/2 + 1/2 = 1.2071 (0.1509 mm,
            // orthographic at 0.125 mm a pixel), at two 2√2 + 1/2 = 3.33, held to 2 (0.25 mm);
            // at one, 1.9142 (0.2393 mm). So orthographic, at half a pixel a point, the edge
            // 0.13 mm behind the top (zb 0.87) is drawn and 0.17 mm (0.83) hidden, where a pixel
            // a point would draw it; at two, 0.245 mm (0.755) is drawn, where a pixel a point
            // would hide it, and 0.26 mm (0.74) hidden. In perspective the reach is that over
            // 800 of the depth along the line to the eye (the view 1 mm high a mm from it):
            // (101 − zb) × (1 − 1.2071/800), 99.979 and 100.019, and (101 − zb) × (1 − 2/800),
            // 99.994 and 100.009, against the top at 100. Drawn, the rows are darker than the
            // top; hidden, the top's.
            for perspective in [false, true] {
                let camera = above_the_wall(perspective);
                for (pixels_per_point, zb, drawn) in [
                    (0.5, 0.87, true),
                    (0.5, 0.83, false),
                    (2.0, 0.755, true),
                    (2.0, 0.74, false),
                ] {
                    r.upload(1, &thin_wall(zb)).unwrap();
                    let image = drawn_image(r, &plain_frame(&camera, SQUARE, pixels_per_point));
                    let line = ImageLine::new([330.0, 400.0], [470.0, 400.0]);
                    let test = if drawn { darker } else { its_face };
                    let got = line.tally(&image, 10.0, (0.0, 0.6), 10.0, test);
                    let what = format!(
                        "zb {zb}, {pixels_per_point} pixels a point, perspective {perspective}, drawn {drawn}"
                    );
                    assert!(all_of(got, 120), "{what}: {got:?} of {SIDES:?}");
                }
            }
        },
    );
}

/// A render's PNG, `size` pixels, as a [`Drawn`].
fn png_image(png: &[u8], size: [u32; 2]) -> Drawn {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut rgb = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut rgb).unwrap();
    Drawn {
        size,
        pixels: rgb
            .chunks_exact(3)
            .map(|p| [p[0], p[1], p[2], 255])
            .collect(),
    }
}

/// Whether a pixel is the highlight's blue, to 3 of 255 a channel.
fn highlight_coloured(pixel: [u8; 4], _: [u8; 4]) -> bool {
    (0..3).all(|k| pixel[k].abs_diff(HIGHLIGHT[k]) <= 3)
}

/// Whether a pixel is bluer than its face (its blue less its red) by 15 of 255 or more:
/// blended with the highlight's blue.
fn tinted(pixel: [u8; 4], face: [u8; 4]) -> bool {
    let blue = |p: [u8; 4]| i32::from(p[2]) - i32::from(p[0]);
    blue(pixel) - blue(face) >= 15
}

#[test]
fn a_highlighted_concave_edge_in_a_render_is_drawn_to_its_plain_strokes_reach_and_no_farther() {
    on_lavapipe(
        "render::frame_tests::a_highlighted_concave_edge_in_a_render_is_drawn_to_its_plain_strokes_reach_and_no_farther",
        |r| {
            // The worker's render of the L ([`l_camera`], 800 by 800, a pixel a point) with
            // Edge1, the concave (5, y, 5), and Edge4, the convex wall top (5, y, 20),
            // highlighted: each drawn 4 pixels wide (2 either side of its line) in the
            // highlight's blue, and moved toward the eye by the plain stroke's reach, √2 + 1/2 =
            // 1.9142 pixels of depth. Each face beside Edge1 rises toward the eye away from its
            // line by √1.5 = 1.2247 pixels of depth a pixel, so its highlight is drawn to
            // 1.9142/1.2247 = 1.563 pixels off its line, of its 2: each pixel whose centre is
            // within 1.2 of it (its samples within 1.5, the faces at most 1.84 nearer) is the
            // highlight's blue, as Edge4's are; each 1.9 to 2.2 off (its samples under the
            // stroke 1.6 off or more, the faces 1.96 nearer or more) is its face's, where
            // Edge4's are tinted (a fifth to a half covered, the stroke's coverage falling
            // from whole half a pixel inside its side to none half a pixel past it). Before N5's image all of it was the faces'; a reach of the
            // highlight's own, 2√2 + 1/2 = 3.33 pixels, would draw it whole. In the ID buffer,
            // (400, 401), its centre 1.05 off Edge1 on the floor's side (the floor 1.29 nearer),
            // is Edge1's (the floor's without the highlight); (400, 402), 1.92 off (2.35
            // nearer), within the stroke's 2 but past its reach, the floor's, Face1; and
            // (400, 397), 2.42 off on the wall's side, past the stroke, the wall's, Face2.
            let scene = silico_protocol::RenderScene {
                bodies: vec![silico_protocol::RenderBody {
                    body: 1,
                    mesh: l_mesh(),
                    highlight_faces: vec![],
                    highlight_edges: vec![0, 3],
                }],
                camera: l_camera(),
                width: SQUARE[0],
                height: SQUARE[1],
                ids: true,
            };
            scene.check().unwrap();
            let rendered = r.render(&scene).unwrap();
            let ids = rendered.ids.unwrap();
            let id = |[x, y]: [u32; 2]| ids[usize::try_from(y * SQUARE[0] + x).unwrap()];
            let edge = silico_protocol::EDGE_BIT | 1;
            for (at, want) in [
                ([400, 399], edge),
                ([400, 400], edge),
                ([400, 401], edge),
                ([400, 402], 1),
                ([400, 397], 2),
            ] {
                assert_eq!(id(at), [1, want], "{at:?}");
            }
            let image = png_image(&rendered.png, SQUARE);
            for (what, z) in [("Edge4, convex", 20.0), ("Edge1, concave", 5.0)] {
                let a = l_view([5.0, 0.0, z], 8.0, 400.0);
                let b = l_view([5.0, 20.0, z], 8.0, 400.0);
                for (y, end) in [(0.0, a), (20.0, b)] {
                    assert_pixel(project(&scene.camera, SQUARE, [5.0, y, z]), end, 1e-4);
                }
                let line = ImageLine::new(a, b);
                let near = line.tally(&image, 10.0, (0.0, 1.2), 4.5, highlight_coloured);
                assert!(
                    all_of(near, 100),
                    "{what} within 1.2: {near:?} of {SIDES:?}"
                );
                let rim = if z == 5.0 { its_face } else { tinted };
                let past = line.tally(&image, 10.0, (1.9, 2.2), 4.5, rim);
                assert!(
                    all_of(past, 25),
                    "{what} 1.9 to 2.2 off: {past:?} of {SIDES:?}"
                );
            }
        },
    );
}

/// A frame taller than it's wide, 800 by 1200: 100 mm over its 1200 rows (1/12 mm a pixel)
/// about the centre (400, 600). Each drawn reach is of its rows, 1.9142/1200 of the view's
/// height, against 1.9142/800 were it taken of its 800 columns.
const TALL: [u32; 2] = [800, 1200];

#[test]
fn a_drawn_edge_behind_a_thin_wall_takes_its_reach_of_the_views_rows_wide_or_tall() {
    on_lavapipe(
        "render::frame_tests::a_drawn_edge_behind_a_thin_wall_takes_its_reach_of_the_views_rows_wide_or_tall",
        |r| {
            // The plate from above, orthographic and 100 mm high (`above_the_wall`), a pixel a
            // point. WIDE, 1200 by 800: 0.125 mm a pixel, Edge13 (x 10 to 30, y 20) along row
            // 400.0 from column 600 − 80 = 520 to 680; its drawn reach 1.9142/800 × 100 =
            // 0.2393 mm, so 0.23 mm behind the top (zb 0.77) it's drawn, rows 399 and 400 (their
            // centres half a pixel off it) the edges' colour from column 540 to 660, and 0.245
            // mm behind (0.755) hidden, the top's. Taken of its 1200 columns the reach would be
            // 0.1595 mm, hiding it at 0.23. TALL, 800 by 1200: 1/12 mm a pixel, Edge13 along
            // row 600.0 from column 400 − 120 = 280 to 520; its reach 1.9142/1200 × 100 =
            // 0.1595 mm, so 0.15 mm behind (zb 0.85) drawn, from column 300 to 500, and 0.17
            // (0.83) hidden. Taken of its 800 columns it would be 0.2393 mm, drawing it at 0.17.
            let camera = above_the_wall(false);
            // Each row's pixels checked: 120 wide (centres 540.5 to 659.5), 200 tall.
            for (size, from, to, least, zb, drawn) in [
                (WIDE, [530.0, 400.0], [670.0, 400.0], 120, 0.77, true),
                (WIDE, [530.0, 400.0], [670.0, 400.0], 120, 0.755, false),
                (TALL, [290.0, 600.0], [510.0, 600.0], 200, 0.85, true),
                (TALL, [290.0, 600.0], [510.0, 600.0], 200, 0.83, false),
            ] {
                let [left, right] = [[10.0, 20.0, zb], [30.0, 20.0, zb]];
                assert_pixel(
                    project(&camera, size, left),
                    [from[0] - 10.0, from[1]],
                    1e-4,
                );
                assert_pixel(project(&camera, size, right), [to[0] + 10.0, to[1]], 1e-4);
                r.upload(1, &thin_wall(zb)).unwrap();
                let image = drawn_image(r, &plain_frame(&camera, size, 1.0));
                let test = if drawn { edge_coloured } else { its_face };
                let got = ImageLine::new(from, to).tally(&image, 10.0, (0.0, 0.6), 10.0, test);
                let what = format!("{size:?}, zb {zb}, drawn {drawn}");
                assert!(all_of(got, least), "{what}: {got:?} of {SIDES:?}");
            }
        },
    );
}

#[test]
fn with_edges_not_picked_an_edge_picks_point_takes_its_reach_of_the_views_rows_in_a_wide_frame() {
    on_lavapipe(
        "render::frame_tests::with_edges_not_picked_an_edge_picks_point_takes_its_reach_of_the_views_rows_in_a_wide_frame",
        |r| {
            // Edge13 on the plate's top (zb = 1) from above, orthographic, in WIDE, 1200 by 800
            // (0.125 mm a pixel about (600, 400)), a pixel a point, edges not picked: the pixel
            // (600, 400), its centre half a pixel right of and below the line's point (20, 20,
            // 1), is its drawn strip's, moved toward the eye by 1.9142/800 of the view's 100 mm,
            // 0.2393 mm, and its point taken back by as much: (20.0625, 19.9375, 1), by hand.
            // Either reach taken of the 1200 columns, 0.1595 mm, would leave z at 1 + 0.2393 −
            // 0.1595 = 1.0798 (the point's) or 1 − 0.0798 = 0.9202 (the strip's).
            r.upload(1, &thin_wall(1.0)).unwrap();
            let camera = above_the_wall(false);
            assert_pixel(
                project(&camera, WIDE, [20.0, 20.0, 1.0]),
                [600.0, 400.0],
                1e-4,
            );
            let asking = Frame {
                size_px: WIDE,
                edges: false,
                ..frame(&camera, &[1], &[], Some(rect([600, 400], [1, 1])))
            };
            let hit = pick(r, &asking).hit.unwrap();
            assert_eq!(
                (hit.body, hit.hit, hit.pixel),
                (1, Element::Edge(13), [600, 400])
            );
            assert_point(&hit, [20.0625, 19.9375, 1.0], [1e-3; 3]);
        },
    );
}

// ---- Lines seen from aside, along a concave edge, in perspective, and at 2 pixels a point
// (#449's review) ----

/// Whether a pixel is the lines' magenta, to 3 of 255 a channel.
fn magenta_coloured(pixel: [u8; 4], _: [u8; 4]) -> bool {
    (0..3).all(|k| pixel[k].abs_diff(MAGENTA[k]) <= 3)
}

/// `strips` in magenta, `width` points wide.
fn magenta_wide(strips: &super::Strips, width: f32) -> super::Lines<'_> {
    super::Lines {
        strips,
        colour: MAGENTA,
        width,
    }
}

#[test]
fn a_line_on_a_face_seen_from_aside_is_drawn_whole_across_its_stroke() {
    on_lavapipe(
        "render::frame_tests::a_line_on_a_face_seen_from_aside_is_drawn_whole_across_its_stroke",
        |r| {
            // A line on the box's top face, (5, 10, 10) to (35, 10, 10), 2 points wide (an
            // edge's), seen from `oblique`'s corner, 40 mm over 800 rows: the face isn't square
            // to the view, so across the line it rises toward the eye on one side. Drawn
            // square to the view at its line's depth, half its stroke would be behind the
            // face; moved toward the eye as an edge is, every pixel whose centre is within
            // half a pixel of the line is magenta on both sides, along its middle.
            r.upload(1, &the_box()).unwrap();
            let camera = oblique();
            let size = [800, 800];
            let (a, b) = ([5.0, 10.0, 10.0], [35.0, 10.0, 10.0]);
            let strips = super::Strips::new([vec![a, b]]).unwrap();
            let lines = [magenta_wide(&strips, 2.0)];
            let image = drawn_image(
                r,
                &Frame {
                    lines: &lines,
                    ..plain_frame(&camera, size, 1.0)
                },
            );
            let line = ImageLine::new(
                project(&camera, size, a).unwrap(),
                project(&camera, size, b).unwrap(),
            );
            let whole = line.tally(&image, 8.0, (0.0, 0.5), 3.5, magenta_coloured);
            assert!(all_of(whole, 100), "whole {whole:?} of {SIDES:?}");
        },
    );
}

#[test]
fn a_line_along_a_concave_edge_is_drawn_whole_over_the_faces_beside_it() {
    on_lavapipe(
        "render::frame_tests::a_line_along_a_concave_edge_is_drawn_whole_over_the_faces_beside_it",
        |r| {
            // The L's concave Edge1, (5, y, 5) for y from 0 to 20, under `l_camera`: both its
            // faces rise toward the eye away from it (the concave edges' test's hand values),
            // so a line along it, 2 points wide, drawn at its line's depth would be hidden on
            // both sides; moved toward the eye as an edge is, it's drawn whole, magenta within
            // half a pixel of the line on both sides, over the L's own edge there too.
            r.upload(1, &Arc::new(l_mesh())).unwrap();
            let camera = l_camera();
            let (a, b) = ([5.0, 0.0, 5.0], [5.0, 20.0, 5.0]);
            let strips = super::Strips::new([vec![a, b]]).unwrap();
            let lines = [magenta_wide(&strips, 2.0)];
            let image = drawn_image(
                r,
                &Frame {
                    lines: &lines,
                    ..plain_frame(&camera, SQUARE, 1.0)
                },
            );
            let line = ImageLine::new(l_view(a, 8.0, 400.0), l_view(b, 8.0, 400.0));
            let whole = line.tally(&image, 8.0, (0.0, 0.5), 3.5, magenta_coloured);
            assert!(all_of(whole, 50), "whole {whole:?} of {SIDES:?}");
        },
    );
}

#[test]
fn a_set_of_two_strips_draws_each_and_nothing_between_and_a_first_strip_off_view_leaves_the_second()
{
    on_lavapipe(
        "render::frame_tests::a_set_of_two_strips_draws_each_and_nothing_between_and_a_first_strip_off_view_leaves_the_second",
        |r| {
            // One set, two strips on row 26 (z = ROW_26_Z, y = 0): x 2 to 12, columns 32 + (x −
            // 21) × 12/11 = 11.27 to 22.18, and x 28 to 38, 39.64 to 50.55. Each is drawn;
            // the columns between, 24 to 37, are the frame without lines. Then a set whose
            // first strip is far off the view, x −300 to −290, and its second x 28 to 38: the
            // second drawn.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let at = |x: f64| [x, 0.0, ROW_26_Z];
            let two =
                super::Strips::new([vec![at(2.0), at(12.0)], vec![at(28.0), at(38.0)]]).unwrap();
            let without = image(r, &frame(&camera, &[1], &[], None));
            let with = image(
                r,
                &Frame {
                    lines: &[magenta(&two)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            assert_magenta_row(&with, 26, 13, 20);
            assert_magenta_row(&with, 26, 41, 49);
            for x in 24..=37 {
                for y in 24..=28 {
                    assert_eq!(pixel(&with, x, y), pixel(&without, x, y), "({x}, {y})");
                }
            }
            let off_first =
                super::Strips::new([vec![at(-300.0), at(-290.0)], vec![at(28.0), at(38.0)]])
                    .unwrap();
            let drawn = image(
                r,
                &Frame {
                    lines: &[magenta(&off_first)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            assert_magenta_row(&drawn, 26, 41, 49);
            for x in 0..=37 {
                assert_eq!(pixel(&drawn, x, 26), pixel(&without, x, 26), "({x}, 26)");
            }
        },
    );
}

#[test]
fn in_perspective_a_line_is_drawn_where_project_puts_it_and_only_its_part_in_front_of_the_eye() {
    on_lavapipe(
        "render::frame_tests::in_perspective_a_line_is_drawn_where_project_puts_it_and_only_its_part_in_front_of_the_eye",
        |r| {
            // From (21, −50, 7) toward +y, 40° high: on the front face, 50 mm from the eye,
            // the view is 2 × 50 tan 20° = 36.397 mm over 48 rows. A line there at z = 7 +
            // 2.5 × 36.397/48 lands on row 21.5, from x 11 to 31: columns 32 ∓ 10 × 48/36.397,
            // 18.81 to 45.19.
            r.upload(1, &the_box()).unwrap();
            let camera = front_in_perspective();
            let size = [WIDTH, HEIGHT];
            let span = 2.0 * 50.0 * libm::tan(20f64.to_radians());
            let z = 7.0 + 2.5 * span / 48.0;
            assert_pixel(project(&camera, size, [21.0, 0.0, z]), [32.0, 21.5], 1e-6);
            let on_face = super::Strips::new([vec![[11.0, 0.0, z], [31.0, 0.0, z]]]).unwrap();
            let drawn = image(
                r,
                &Frame {
                    lines: &[magenta(&on_face)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            assert_magenta_row(&drawn, 21, 20, 44);
            // A line at the eye's height, x = 26, from the face (y = 0) to 50 mm behind the
            // eye (y = −100), 3 points wide: in front of the eye it runs from column 32 +
            // 5 × 48/(2 d tan 20°) = 32 + 329.7/d at depth d, 38.59 at the face, toward the
            // right edge (column 63 at d = 10.6), on rows 23 and 24 (z = 7 is row 24, the
            // eye's height at every depth), 3 pixels wide at every depth: 22.5 to 25.5. It's
            // cut to the view's pyramid before it's drawn (`View::cut_strip`). The GPU would
            // clip an uncut strip in clip space all the same, but re_renderer sizes a stroke at
            // each of its points by the point's distance from the camera, clamped to 0, so an
            // uncut strip tapers behind the eye and widens toward it: every pixel off rows 22
            // to 25 from column 37 on is the frame without lines. (The floor the cut keeps,
            // MIN_EYE_DISTANCE, is pinned by the edges' tests, through the same `cut_strip`.)
            assert_pixel(
                project(&camera, size, [26.0, 0.0, 7.0]),
                [32.0 + 5.0 * 48.0 / span, 24.0],
                1e-6,
            );
            let through =
                super::Strips::new([vec![[26.0, 0.0, 7.0], [26.0, -100.0, 7.0]]]).unwrap();
            let without = image(r, &frame(&camera, &[1], &[], None));
            let crossing = image(
                r,
                &Frame {
                    lines: &[magenta_wide(&through, 3.0)],
                    ..frame(&camera, &[1], &[], None)
                },
            );
            for y in [23, 24] {
                for x in 40..=62 {
                    let got = pixel(&crossing, x, y);
                    assert!(within(got, MAGENTA, 8), "({x}, {y}): {got:?}");
                }
            }
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    if !((22..=25).contains(&y) && x >= 37) {
                        assert_eq!(pixel(&crossing, x, y), pixel(&without, x, y), "({x}, {y})");
                    }
                }
            }
        },
    );
}

#[test]
fn at_two_pixels_a_point_a_line_is_three_pixels_wide_on_its_rows_and_nowhere_else() {
    on_lavapipe(
        "render::frame_tests::at_two_pixels_a_point_a_line_is_three_pixels_wide_on_its_rows_and_nowhere_else",
        |r| {
            // `front`, 128 by 96 pixels at 2 a point: 44 mm over 96 rows. The line at z =
            // ROW_26_Z lands on row 48 + 2.5 × 11/12 × 96/44 = 53.0, 1.5 points (3 pixels) wide:
            // rows 52 and 53, whose centres are half a pixel off it, whole; rows 51 and 54 in
            // part; every other row the frame without lines. Columns 64 + (x − 21) × 96/44 for x
            // 2 to 38: 22.55 to 101.09.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let size = [2 * WIDTH, 2 * HEIGHT];
            assert_pixel(
                project(&camera, size, [21.0, 0.0, ROW_26_Z]),
                [64.0, 53.0],
                1e-6,
            );
            let line = line_at(0.0, ROW_26_Z);
            let plain = Frame {
                size_px: size,
                pixels_per_point: 2.0,
                ..frame(&camera, &[1], &[], None)
            };
            let commands = r.prepare(&plain).unwrap();
            let without = composited_sized(r, Some(commands), wgpu::Color::TRANSPARENT, size);
            let lines = [magenta(&line)];
            let commands = r
                .prepare(&Frame {
                    lines: &lines,
                    ..plain
                })
                .unwrap();
            let with = composited_sized(r, Some(commands), wgpu::Color::TRANSPARENT, size);
            let at = |image: &[[u8; 4]], x: u32, y: u32| {
                image[usize::try_from(y * size[0] + x).unwrap()]
            };
            for y in [52, 53] {
                for x in 25..=99 {
                    let got = at(&with, x, y);
                    assert!(within(got, MAGENTA, 8), "({x}, {y}): {got:?}");
                }
            }
            for y in (0..size[1]).filter(|y| !(51..=54).contains(y)) {
                for x in 0..size[0] {
                    assert_eq!(at(&with, x, y), at(&without, x, y), "({x}, {y})");
                }
            }
        },
    );
}

#[test]
fn a_set_whose_corner_reaches_too_far_is_taken_by_its_points() {
    on_lavapipe(
        "render::frame_tests::a_set_whose_corner_reaches_too_far_is_taken_by_its_points",
        |r| {
            // `front` takes a reach of at most 44 × 2^19 / 48 = 480,597 mm. A set of (21 + 4e5,
            // 10, 7) and (21, 10 + 4e5, 7): its box's far corner (4e5, 4e5, 0) off the target,
            // 565,685 mm, is refused; its points, each 4e5 mm off the target, aren't. So the
            // frame is taken, its depth the points' reach with 1% of it clear, and Select
            // Other's cast's near plane the same.
            let camera = front();
            let set = super::Strips::new([vec![[21.0 + 4e5, 10.0, 7.0], [21.0, 10.0 + 4e5, 7.0]]])
                .unwrap();
            let CheckedFrame { view, .. } = r
                .check_frame(&Frame {
                    lines: &[magenta(&set)],
                    ..frame(&camera, &[], &[], None)
                })
                .unwrap();
            assert!((view.depth - 4e5 * 1.01).abs() < 1e-6, "{}", view.depth);
            let cast = super::Caster::new(&camera, [WIDTH, HEIGHT], 1.0, &[], &[&set]).unwrap();
            assert!(
                (cast.near.unwrap() - 4e5 * 1.01).abs() < 1e-6,
                "{:?}",
                cast.near
            );
        },
    );
}

#[test]
fn a_frames_lines_at_their_most_points_are_drawn_with_its_edge_still_picked_and_one_more_refused() {
    on_lavapipe(
        "render::frame_tests::a_frames_lines_at_their_most_points_are_drawn_with_its_edge_still_picked_and_one_more_refused",
        |r| {
            // A million strips of two points, each half a millimetre along x on the front face
            // between z 1 and 4 (rows 27.3 to 30.5, clear of the top edge at row 20.73): the
            // frame's most points, MAX_FRAME_LINE_POINTS. Prepared, drawn, and picked, the top
            // edge picked where it is: a frame at the most works. (Not that an edge's pick
            // can't be crowded out: the picks are put in after the lines.) A set of one more
            // strip beside them: past the most, refused, naming the lines.
            assert_eq!(super::MAX_FRAME_LINE_POINTS, 2_000_000);
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let many = super::Strips::new((0..1_000_000u32).map(|k| {
                let x = 2.0 + f64::from(k % 70) * 0.5;
                let z = 1.0 + f64::from(k / 70 % 4);
                vec![[x, 0.0, z], [x + 0.5, 0.0, z]]
            }))
            .unwrap();
            assert_eq!(many.points(), super::MAX_FRAME_LINE_POINTS);
            let at_most = [magenta(&many)];
            let found = pick(
                r,
                &Frame {
                    lines: &at_most,
                    ..frame(&camera, &[1], &[], Some(ON_THE_TOP_EDGE))
                },
            )
            .hit
            .unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Edge(3), [32, 20])
            );
            let one = line_at(0.0, ROW_26_Z);
            let past = [magenta(&many), magenta(&one)];
            assert_eq!(
                refused(
                    r,
                    &Frame {
                        lines: &past,
                        ..frame(&camera, &[1], &[], None)
                    }
                ),
                "lines"
            );
        },
    );
}

// ---- How far a line is moved toward the eye, and its tie with an edge's pick (#449's
// second review) ----

/// The box from the front at 256 by 192 pixels: [`front`]'s camera, 44 mm over 192 rows,
/// 11/48 mm a pixel about (128, 96). From 167 rows an edge's picking reach is its 2 pixels
/// of depth; below, it's `VERTEX_REACH` of the view's height (0.576 pixels at 48 rows),
/// and so is every line's, whatever its width.
const ROWS_192: [u32; 2] = [256, 192];

#[test]
fn a_line_just_behind_a_face_is_drawn_within_its_strokes_reach_and_hidden_past_it() {
    on_lavapipe(
        "render::frame_tests::a_line_just_behind_a_face_is_drawn_within_its_strokes_reach_and_hidden_past_it",
        |r| {
            // At ROWS_192, 1.5 points wide at a pixel a point, a line is moved toward the eye by
            // √2 × 0.75 + 1/2 = 1.561 pixels of depth (drawn_stroke_fraction), under an edge's
            // pick's 2; 4 points wide, by √2 × 2 + 1/2 = 3.33, held to the 2. A line along x
            // inside the box, d pixels behind its front face (y = d × 11/48 mm), at z = 7 −
            // 10.5 × 11/48 (row 106.5, row 106's centre), over columns 128 + (x − 21) × 48/11
            // for x 2 to 38, 45.1 to 202.2: at d = 1.35, 1.5 points wide, it's moved 0.21
            // pixels in front of the face, and drawn; at d = 1.8 it stays 0.24 behind it, and
            // the frame is the frame without lines, byte for byte; 4 points wide at d = 1.8,
            // it's moved 0.2 in front, and drawn. An edge's reach whatever the width (1.914)
            // would draw the second; a reach of 1.2 pixels would hide the first and the third.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let mm = 44.0 / 192.0;
            let z = 7.0 - 10.5 * mm;
            assert_pixel(
                project(&camera, ROWS_192, [21.0, 0.0, z]),
                [128.0, 106.5],
                1e-6,
            );
            let without = drawn_image(r, &plain_frame(&camera, ROWS_192, 1.0));
            let drawn = |r: &mut Renderer, d: f64, width: f32| {
                let strips = line_at(d * mm, z);
                drawn_image(
                    r,
                    &Frame {
                        lines: &[magenta_wide(&strips, width)],
                        ..plain_frame(&camera, ROWS_192, 1.0)
                    },
                )
            };
            for (d, width) in [(1.35, 1.5), (1.8, 4.0)] {
                let image = drawn(r, d, width);
                for x in 60..=190 {
                    let got = image.at([x, 106]);
                    assert!(
                        within(got, MAGENTA, 8),
                        "{d} behind, {width} wide, ({x}, 106): {got:?}"
                    );
                }
            }
            assert!(
                drawn(r, 1.8, 1.5).pixels == without.pixels,
                "1.8 behind, 1.5 wide: drawn"
            );
            // At 2 pixels a point, 512 by 384 (44 mm over 384 rows, 11/96 mm a pixel, z = 7 on
            // row 192), 1.5 points is 3 pixels: √2 × 1.5 + 1/2 = 2.62 pixels of depth, held to
            // the 2. The line at d = 1.8 (y = 1.8 × 11/96), z = 7 − 20.5 × 11/96 (row 212.5),
            // over columns 256 + (x − 21) × 96/11, 90.2 to 404.4: moved 0.2 in front of the
            // face, and drawn. Taken at a pixel a point (1.561) it would stay behind.
            let size = [512, 384];
            let mm = 44.0 / 384.0;
            let z = 7.0 - 20.5 * mm;
            assert_pixel(project(&camera, size, [21.0, 0.0, z]), [256.0, 212.5], 1e-6);
            let strips = line_at(1.8 * mm, z);
            let image = drawn_image(
                r,
                &Frame {
                    lines: &[magenta_wide(&strips, 1.5)],
                    ..plain_frame(&camera, size, 2.0)
                },
            );
            for x in 120..=380 {
                let got = image.at([x, 212]);
                assert!(
                    within(got, MAGENTA, 8),
                    "1.8 behind at 2 pixels a point, ({x}, 212): {got:?}"
                );
            }
        },
    );
}

#[test]
fn a_line_along_an_edge_as_near_the_eye_as_its_pick_leaves_the_edge_picked() {
    on_lavapipe(
        "render::frame_tests::a_line_along_an_edge_as_near_the_eye_as_its_pick_leaves_the_edge_picked",
        |r| {
            // At ROWS_192, the front face's top edge (Edge3, z = 10) lies on row 96 − 3 × 48/11 =
            // 82.91: row 82's centre, 0.41 from it, is inside its picking strip's half width
            // (1 pixel) and above the face. A line along it 4 points wide is moved toward the
            // eye by 2 pixels of depth, an edge's picking reach, as the edge's picking strip
            // is, along x as the edge is: the same depth. The edge's picks are put in after
            // the lines, so of the two the edge's is picked there.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            assert_pixel(
                project(&camera, ROWS_192, [21.0, 0.0, 10.0]),
                [128.0, 96.0 - 3.0 * 48.0 / 11.0],
                1e-6,
            );
            let on_edge = line_at(0.0, 10.0);
            let lines = [magenta_wide(&on_edge, 4.0)];
            let found = pick(
                r,
                &Frame {
                    lines: &lines,
                    picking: Some(PixelRect {
                        min: [128, 82],
                        size: [1, 1],
                    }),
                    ..plain_frame(&camera, ROWS_192, 1.0)
                },
            )
            .hit
            .unwrap();
            assert_eq!(
                (found.body, found.hit, found.pixel),
                (1, Element::Edge(3), [128, 82])
            );
        },
    );
}

// ---- Strips' own reads, and the frame's log of its lines (#449's mutants) ----

#[test]
fn strips_say_whether_they_hold_none_and_print_their_counts_and_box_not_their_points() {
    let none = super::Strips::new(Vec::<Vec<[f64; 3]>>::new()).unwrap();
    assert!(none.is_empty());
    assert_eq!(
        format!("{none:?}"),
        "Strips { strips: 0, points: 0, bounds: None }"
    );
    // Five points over two strips, their box (0, 0, 0) to (4, 2, 1); the middle point's 1.25
    // and 0.5, no corner of it, aren't printed.
    let two = super::Strips::new([
        vec![[0.0, 0.0, 0.0], [1.25, 2.0, 0.5], [4.0, 0.0, 1.0]],
        vec![[0.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
    ])
    .unwrap();
    assert!(!two.is_empty());
    assert_eq!(
        format!("{two:?}"),
        "Strips { strips: 2, points: 5, bounds: Some(([0.0, 0.0, 0.0], [4.0, 2.0, 1.0])) }"
    );
}

/// The `sets` and `strips` of each `lines` event `f` logs, in order.
fn lines_events(f: impl FnOnce()) -> Vec<(u64, u64)> {
    logged("lines", ["sets", "strips"], f)
}

/// The two fields `names` of each event `f` logs whose message is `message`, in order (each
/// `u64::MAX` if it hasn't it).
fn logged(message: &'static str, names: [&'static str; 2], f: impl FnOnce()) -> Vec<(u64, u64)> {
    use std::sync::Mutex;
    use tracing_subscriber::layer::{Context, SubscriberExt};

    struct Fields {
        names: [&'static str; 2],
        message: String,
        values: [Option<u64>; 2],
    }
    impl tracing::field::Visit for Fields {
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            for (name, slot) in self.names.iter().zip(&mut self.values) {
                if field.name() == *name {
                    *slot = Some(value);
                }
            }
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.message = format!("{value:?}");
            }
        }
    }
    struct Events {
        message: &'static str,
        names: [&'static str; 2],
        seen: Arc<Mutex<Vec<(u64, u64)>>>,
    }
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Events {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            let mut fields = Fields {
                names: self.names,
                message: String::new(),
                values: [None; 2],
            };
            event.record(&mut fields);
            if fields.message == self.message {
                let [a, b] = fields.values.map(|v| v.unwrap_or(u64::MAX));
                self.seen.lock().unwrap().push((a, b));
            }
        }
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(Events {
        message,
        names,
        seen: Arc::clone(&seen),
    });
    tracing::subscriber::with_default(subscriber, f);
    seen.lock().unwrap().clone()
}

#[test]
fn a_frames_lines_are_logged_by_their_sets_and_the_strips_drawn_of_them() {
    on_lavapipe(
        "render::frame_tests::a_frames_lines_are_logged_by_their_sets_and_the_strips_drawn_of_them",
        |r| {
            // Two sets: the first a strip on the front face and one from x = 300 to 310, off
            // the view (58.67 mm wide about x = 21, so x −8.33 to 50.33), which isn't drawn;
            // the second a strip on the face. Logged once: 2 sets, 2 strips drawn. A frame of
            // no lines logs none.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let first = super::Strips::new([
                vec![[2.0, 0.0, ROW_26_Z], [38.0, 0.0, ROW_26_Z]],
                vec![[300.0, 0.0, ROW_26_Z], [310.0, 0.0, ROW_26_Z]],
            ])
            .unwrap();
            let second = line_at(0.0, 2.0);
            let lines = [magenta(&first), magenta(&second)];
            let logged = lines_events(|| {
                draw(
                    r,
                    &Frame {
                        lines: &lines,
                        ..frame(&camera, &[1], &[], None)
                    },
                );
            });
            assert_eq!(logged, [(2, 2)]);
            let none = lines_events(|| draw(r, &frame(&camera, &[1], &[], None)));
            assert!(none.is_empty(), "{none:?}");
        },
    );
}

// Planes among the bodies (Frame::planes: the app's origin planes, the approved prototype's
// `buildPlanes`). Each expected pixel is placed by the front camera's rule, by hand: x = 21
// + (column − 32) × 11/12 and z = 7 + (24 − row) × 11/12 (44 mm over 48 rows, about the
// target (21, 10, 7)), and each blend worked in linear light by the sRGB formula: a fill of
// alpha a over what's there, `(1 − a) · under + a · fill`.

/// x at the centre of column `column`, and z at the centre of row `row`, as [`front`] draws
/// them.
fn x_at(column: f64) -> f64 {
    21.0 + (column - 32.0) * 11.0 / 12.0
}

fn z_at(row: f64) -> f64 {
    7.0 + (24.0 - row) * 11.0 / 12.0
}

/// A plane square to [`front`]'s view at `y`, its sides through the centres of columns 20
/// and 44 and rows 18 and 30: x from 10.458 to 32.458, z from 12.042 to 1.042.
fn facing(y: f64, fill: [u8; 4], outline: [u8; 4]) -> super::Plane {
    facing_over(y, [20, 18, 44, 30], fill, outline)
}

/// A plane square to [`front`]'s view at `y`, its sides through the centres of the columns
/// `left` and `right` and the rows `top` and `bottom`.
fn facing_over(
    y: f64,
    [left, top, right, bottom]: [u32; 4],
    fill: [u8; 4],
    outline: [u8; 4],
) -> super::Plane {
    let centre = |c: u32| f64::from(c) + 0.5;
    let (left, right) = (x_at(centre(left)), x_at(centre(right)));
    let (top, bottom) = (z_at(centre(top)), z_at(centre(bottom)));
    super::Plane {
        origin: [left, y, bottom],
        u: [right - left, 0.0, 0.0],
        v: [0.0, 0.0, top - bottom],
        fill,
        outline,
        width: 1.0,
    }
}

/// Half-translucent magenta, which neither the body's grey nor its edges can be.
const HALF_MAGENTA: [u8; 4] = [255, 0, 255, 128];
const GREEN: [u8; 4] = [0, 255, 0, 255];
/// Nothing: a fill or an outline of alpha 0 isn't drawn.
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// What `colour` (its alpha unmultiplied) over the opaque pixel `under` shows, in sRGB, by
/// hand.
fn over(under: [u8; 4], colour: [u8; 4]) -> [f64; 3] {
    let a = f64::from(colour[3]) / 255.0;
    std::array::from_fn(|k| srgb_of((1.0 - a) * linear(under[k]) + a * linear(colour[k])))
}

#[test]
fn a_planes_fill_lands_where_the_projection_puts_it_over_what_is_behind_it() {
    on_lavapipe(
        "render::frame_tests::a_planes_fill_lands_where_the_projection_puts_it_over_what_is_behind_it",
        |r| {
            let camera = front();
            // Its corners land on the pixels' centres the rule puts them at.
            let plane = facing(-5.0, HALF_MAGENTA, CLEAR);
            let [low, _, high, _] = plane.corners();
            assert_pixel(project(&camera, [WIDTH, HEIGHT], low), [20.5, 30.5], 1e-6);
            assert_pixel(project(&camera, [WIDTH, HEIGHT], high), [44.5, 18.5], 1e-6);
            // No body: an orthographic view's reach takes the plane's corners, so it's drawn.
            let without = image(r, &frame(&camera, &[], &[], None));
            let planes = [plane];
            let with = image(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[], &[], None)
                },
            );
            let want = over(BLACK, HALF_MAGENTA);
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let inside = (21..=43).contains(&x) && (19..=29).contains(&y);
                    let clear = !(19..=45).contains(&x) || !(17..=31).contains(&y);
                    if inside {
                        assert_colour(pixel(&with, x, y), want, 3.0, &format!("({x}, {y})"));
                    } else if clear {
                        assert_eq!(pixel(&with, x, y), pixel(&without, x, y), "({x}, {y})");
                    }
                }
            }
            assert_eq!(pixel(&without, 32, 24), BLACK);
        },
    );
}

#[test]
fn a_planes_outline_is_drawn_along_its_sides_and_nowhere_else() {
    on_lavapipe(
        "render::frame_tests::a_planes_outline_is_drawn_along_its_sides_and_nowhere_else",
        |r| {
            // Opaque green, a point wide, its fill clear: its sides through the centres of
            // columns 20 and 44 and rows 18 and 30 cover those pixels whole (a line's coverage
            // is 1 at its middle), and a pixel's centre one off is past its half width.
            let camera = front();
            let planes = [facing(-5.0, CLEAR, GREEN)];
            let with = image(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[], &[], None)
                },
            );
            let near = |got: [u8; 4]| within(got, GREEN, 8);
            for y in 20..=28 {
                for x in [20, 44] {
                    assert!(
                        near(pixel(&with, x, y)),
                        "({x}, {y}): {:?}",
                        pixel(&with, x, y)
                    );
                }
            }
            for x in 22..=42 {
                for y in [18, 30] {
                    assert!(
                        near(pixel(&with, x, y)),
                        "({x}, {y}): {:?}",
                        pixel(&with, x, y)
                    );
                }
            }
            // Inside it and outside it, a pixel or more past the stroke: the background.
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let on_a_side = ((19..=21).contains(&x) || (43..=45).contains(&x))
                        && (17..=31).contains(&y)
                        || ((17..=19).contains(&y) || (29..=31).contains(&y))
                            && (19..=45).contains(&x);
                    if !on_a_side {
                        assert_eq!(pixel(&with, x, y), BLACK, "({x}, {y})");
                    }
                }
            }
        },
    );
}

#[test]
fn a_body_in_front_hides_a_plane_and_a_plane_in_front_tints_the_body_and_hides_nothing() {
    on_lavapipe(
        "render::frame_tests::a_body_in_front_hides_a_plane_and_a_plane_in_front_tints_the_body_and_hides_nothing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let without = image(r, &frame(&camera, &[1], &[], None));
            // Behind the box (its back face at y = 20), from row 10 down: where the box is
            // (its front face rows 20.73 to 31.64), the frame without the plane, byte for
            // byte; above it and its top edge's stroke (rows 11 to 17, z 19.0 to 12.6, over
            // 10), the plane over the background.
            let behind = [facing_over(25.0, [20, 10, 44, 30], HALF_MAGENTA, CLEAR)];
            let hidden = image(
                r,
                &Frame {
                    planes: &behind,
                    ..frame(&camera, &[1], &[], None)
                },
            );
            for y in 22..=29 {
                for x in 21..=43 {
                    assert_eq!(pixel(&hidden, x, y), pixel(&without, x, y), "({x}, {y})");
                }
            }
            for y in 11..=17 {
                assert_colour(
                    pixel(&hidden, 32, y),
                    over(BLACK, HALF_MAGENTA),
                    3.0,
                    &format!("(32, {y}), above the box"),
                );
            }
            // In front of it (y = −5): the face under it is its own colour mixed with the
            // fill's, not hidden; the face beside the plane as it was.
            let ahead = [facing(-5.0, HALF_MAGENTA, CLEAR)];
            let tinted = image(
                r,
                &Frame {
                    planes: &ahead,
                    ..frame(&camera, &[1], &[], None)
                },
            );
            let face = pixel(&without, 32, 26);
            assert!(!within(face, BLACK, 8), "the face {face:?}");
            for (x, y) in [(32, 26), (24, 23), (40, 28)] {
                let under = pixel(&without, x, y);
                assert_colour(
                    pixel(&tinted, x, y),
                    over(under, HALF_MAGENTA),
                    3.0,
                    &format!("({x}, {y}) over {under:?}"),
                );
            }
            for (x, y) in [(12, 26), (50, 26), (32, 31)] {
                assert_eq!(pixel(&tinted, x, y), pixel(&without, x, y), "({x}, {y})");
            }
        },
    );
}

/// Every pixel's ID of `asking`'s frame (its picking set to the whole frame, at most 256
/// a side), as its picking reads them back.
fn ids_of(r: &mut Renderer, asking: &Frame<'_>) -> Vec<[u64; 2]> {
    let asking = Frame {
        picking: Some(rect([0, 0], asking.size_px)),
        ..*asking
    };
    pick(r, &asking);
    draw(r, &asking);
    draw(
        r,
        &Frame {
            serial: asking.serial.wrapping_add(1),
            picking: None,
            ..asking
        },
    );
    r.harvest();
    let answer = r.picked.take().expect("no answer");
    answer
        .ids
        .iter()
        .map(|id| [id.object.0, id.instance.0])
        .collect()
}

/// The three origin planes as the app draws them, their square from −4 to 46 in their own
/// (U, V) (`crate::planes` in the app), in the app's colours (the theme's), the hover's on
/// `lit`.
fn origin_planes(lit: Option<usize>) -> [super::Plane; 3] {
    let sides = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ];
    std::array::from_fn(|i| {
        let (u, v): ([f64; 3], [f64; 3]) = sides[i];
        let hovered = lit == Some(i);
        super::Plane {
            origin: std::array::from_fn(|k| -4.0 * (u[k] + v[k])),
            u: u.map(|c| 50.0 * c),
            v: v.map(|c| 50.0 * c),
            fill: if hovered {
                [0x18, 0x6a, 0xdd, 82]
            } else {
                [0x9f, 0x9f, 0x9f, 20]
            },
            outline: if hovered {
                [0x46, 0x76, 0xff, 153]
            } else {
                [0x9f, 0x9f, 0x9f, 153]
            },
            width: 1.0,
        }
    })
}

#[test]
fn the_picking_layer_is_the_same_with_planes_as_without_pixel_for_pixel() {
    on_lavapipe(
        "render::frame_tests::the_picking_layer_is_the_same_with_planes_as_without_pixel_for_pixel",
        |r| {
            r.upload(1, &the_box()).unwrap();
            // A plane in front of the box over its front face, its edges, and the background
            // beside it; the three origin planes, through the box; and both, from the front
            // and from the corner, orthographic and in perspective, edges and vertices
            // picked: every ID as it is without them.
            let ahead = facing(-5.0, HALF_MAGENTA, GREEN);
            let [xy, xz, yz] = origin_planes(Some(1));
            let all = [ahead, xy, xz, yz];
            let cameras = [
                front(),
                front_in_perspective(),
                from_the_corner(Projection::Orthographic { view_height: 60.0 }),
                from_the_corner(Projection::Perspective {
                    field_of_view: 50.0,
                }),
            ];
            for camera in &cameras {
                let plain = Frame {
                    vertices: Some(4.5),
                    ..frame(camera, &[1], &[], None)
                };
                let without = ids_of(r, &plain);
                assert!(
                    without.iter().any(|id| id[0] == 1),
                    "{camera:?}: the box is in view"
                );
                for planes in [&all[..1], &all[1..], &all[..]] {
                    let with = ids_of(r, &Frame { planes, ..plain });
                    assert!(with == without, "{camera:?}, {} planes", planes.len());
                }
            }
            // And a pick over the plane in front of the face takes the face; over the plane
            // beside the box, nothing.
            let camera = front();
            let planes = [ahead];
            assert_eq!(
                hit(
                    r,
                    &Frame {
                        planes: &planes,
                        ..frame(&camera, &[1], &[], Some(ON_THE_FACE))
                    }
                ),
                Some((1, Element::Face(3), [32, 26]))
            );
            let beside = pick(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[1], &[], Some(rect([30, 17], [3, 3])))
                },
            );
            assert_eq!((beside.hit, beside.edge), (None, None));
        },
    );
}

// Hostile planes (/break-it).

#[test]
fn hostile_planes_are_refused_naming_planes_and_leave_the_renderer_drawing() {
    on_lavapipe(
        "render::frame_tests::hostile_planes_are_refused_naming_planes_and_leave_the_renderer_drawing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let good = facing(-5.0, HALF_MAGENTA, GREEN);
            let far = silico_protocol::MAX_RENDER_COORDINATE;
            let bad = [
                super::Plane {
                    origin: [f64::NAN, 0.0, 0.0],
                    ..good
                },
                super::Plane {
                    u: [f64::INFINITY, 0.0, 0.0],
                    ..good
                },
                // Its corner within the bound, the one across from it past it.
                super::Plane {
                    origin: [far, 0.0, 0.0],
                    u: [1.0, 0.0, 0.0],
                    ..good
                },
                super::Plane {
                    v: [0.0, 0.0, -2.0 * far],
                    ..good
                },
                super::Plane {
                    fill: [10, 20, 30, 255],
                    ..good
                },
                super::Plane { width: 0.0, ..good },
                super::Plane {
                    width: -1.0,
                    ..good
                },
                super::Plane {
                    width: f32::NAN,
                    ..good
                },
                super::Plane {
                    width: super::MAX_LINE_WIDTH.next_up(),
                    ..good
                },
            ];
            for plane in bad {
                let planes = [good, plane];
                assert_eq!(
                    refused(
                        r,
                        &Frame {
                            planes: &planes,
                            ..frame(&camera, &[1], &[], None)
                        }
                    ),
                    "planes",
                    "{plane:?}"
                );
                // And drawing after it.
                draw(r, &frame(&camera, &[1], &[], None));
            }
            // As many as a frame takes, drawn; one more, refused.
            let most = vec![good; super::MAX_FRAME_PLANES];
            draw(
                r,
                &Frame {
                    planes: &most,
                    ..frame(&camera, &[1], &[], None)
                },
            );
            let more = vec![good; super::MAX_FRAME_PLANES + 1];
            assert_eq!(
                refused(
                    r,
                    &Frame {
                        planes: &more,
                        ..frame(&camera, &[1], &[], None)
                    }
                ),
                "planes"
            );
            // A degenerate plane (its sides along one line, or none) is drawn as nothing,
            // and a plane all clear as nothing.
            for plane in [
                super::Plane { v: good.u, ..good },
                super::Plane {
                    u: [0.0; 3],
                    v: [0.0; 3],
                    ..good
                },
                super::Plane {
                    fill: CLEAR,
                    outline: CLEAR,
                    ..good
                },
            ] {
                let planes = [plane];
                draw(
                    r,
                    &Frame {
                        planes: &planes,
                        ..frame(&camera, &[1], &[], None)
                    },
                );
            }
        },
    );
}

#[test]
fn a_plane_through_a_perspective_eye_is_seen_edge_on_and_draws_nothing() {
    on_lavapipe(
        "render::frame_tests::a_plane_through_a_perspective_eye_is_seen_edge_on_and_draws_nothing",
        |r| {
            r.upload(1, &the_box()).unwrap();
            // The eye (21, −50, 7) inside a plane's square (y from −60 to −40, x from 11 to
            // 31, at z = 7): the plane holds the eye and the view's axis, so it's seen edge on.
            let camera = front_in_perspective();
            let planes = [super::Plane {
                origin: [11.0, -60.0, 7.0],
                u: [20.0, 0.0, 0.0],
                v: [0.0, 20.0, 0.0],
                fill: HALF_MAGENTA,
                outline: GREEN,
                width: 1.0,
            }];
            let without = image(r, &frame(&camera, &[1], &[], None));
            let with = image(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[1], &[], None)
                },
            );
            // At most its row through the eye's height, row 24 (the view's axis between
            // rows 23 and 24): every other pixel the frame's own.
            for y in (0..HEIGHT).filter(|y| !(23..=24).contains(y)) {
                for x in 0..WIDTH {
                    assert_eq!(pixel(&with, x, y), pixel(&without, x, y), "({x}, {y})");
                }
            }
            let plain = frame(&camera, &[1], &[], None);
            assert!(
                ids_of(
                    r,
                    &Frame {
                        planes: &planes,
                        ..plain
                    }
                ) == ids_of(r, &plain)
            );
        },
    );
}

#[test]
fn a_perspective_eye_just_before_a_plane_between_its_corners_sees_it_fill_the_view() {
    on_lavapipe(
        "render::frame_tests::a_perspective_eye_just_before_a_plane_between_its_corners_sees_it_fill_the_view",
        |r| {
            r.upload(1, &the_box()).unwrap();
            // A plane at y = −49, 1 mm before the eye (21, −50, 7), from x 0 to 40 and z −10
            // to 30: 40° high, the view there is 0.73 mm high and 0.97 wide, wholly inside it,
            // its outline 20 mm off. Every pixel the box behind it tinted, none hidden.
            let camera = front_in_perspective();
            let planes = [super::Plane {
                origin: [0.0, -49.0, -10.0],
                u: [40.0, 0.0, 0.0],
                v: [0.0, 0.0, 40.0],
                fill: HALF_MAGENTA,
                outline: GREEN,
                width: 1.0,
            }];
            let without = image(r, &frame(&camera, &[1], &[], None));
            let with = image(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[1], &[], None)
                },
            );
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let under = pixel(&without, x, y);
                    assert_colour(
                        pixel(&with, x, y),
                        over(under, HALF_MAGENTA),
                        3.0,
                        &format!("({x}, {y}) over {under:?}"),
                    );
                }
            }
            let plain = frame(&camera, &[1], &[], None);
            assert!(
                ids_of(
                    r,
                    &Frame {
                        planes: &planes,
                        ..plain
                    }
                ) == ids_of(r, &plain)
            );
        },
    );
}

#[test]
fn a_plane_reaching_behind_a_perspective_eye_is_drawn_below_the_horizon_alone() {
    on_lavapipe(
        "render::frame_tests::a_plane_reaching_behind_a_perspective_eye_is_drawn_below_the_horizon_alone",
        |r| {
            // The ground (z = 0) from x −1000 to 1000 and y −1000 to 1000, the eye (21, −50,
            // 7) looking level along +y, 40° high: a ground point d ahead is seen 7/d below
            // the axis, at row 24 + 24 · (7/d) / tan 20°, so every row of it is below 24
            // however far it is, and the rows above the horizon are the background's however
            // much of it is behind the eye. Row 47 (its centre 23.5 below the axis) sees it
            // 19.6 mm ahead, inside it.
            let camera = front_in_perspective();
            let planes = [super::Plane {
                origin: [-1000.0, -1000.0, 0.0],
                u: [2000.0, 0.0, 0.0],
                v: [0.0, 2000.0, 0.0],
                fill: HALF_MAGENTA,
                outline: GREEN,
                width: 1.0,
            }];
            let with = image(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[], &[], None)
                },
            );
            for y in 0..24 {
                for x in 0..WIDTH {
                    assert_eq!(pixel(&with, x, y), BLACK, "({x}, {y})");
                }
            }
            let want = over(BLACK, HALF_MAGENTA);
            for y in 26..HEIGHT {
                for x in 0..WIDTH {
                    assert_colour(pixel(&with, x, y), want, 3.0, &format!("({x}, {y})"));
                }
            }
        },
    );
}

#[test]
fn a_plane_edge_on_to_an_orthographic_view_changes_no_pick() {
    on_lavapipe(
        "render::frame_tests::a_plane_edge_on_to_an_orthographic_view_changes_no_pick",
        |r| {
            r.upload(1, &the_box()).unwrap();
            // The XY plane at z = 5 seen from the front: a line across the box's front face
            // at row 26.18 (its outline's sides along y), no area. Drawn, every pick as it was.
            let camera = front();
            let planes = [super::Plane {
                origin: [-4.0, -4.0, 5.0],
                u: [50.0, 0.0, 0.0],
                v: [0.0, 50.0, 0.0],
                fill: HALF_MAGENTA,
                outline: GREEN,
                width: 1.0,
            }];
            let without = image(r, &frame(&camera, &[1], &[], None));
            let with = image(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[1], &[], None)
                },
            );
            for y in (0..HEIGHT).filter(|y| !(25..=27).contains(y)) {
                for x in 0..WIDTH {
                    assert_eq!(pixel(&with, x, y), pixel(&without, x, y), "({x}, {y})");
                }
            }
            let plain = frame(&camera, &[1], &[], None);
            assert!(
                ids_of(
                    r,
                    &Frame {
                        planes: &planes,
                        ..plain
                    }
                ) == ids_of(r, &plain)
            );
        },
    );
}

#[test]
fn a_camera_far_from_the_planes_draws_them_or_refuses_a_view_too_small_for_their_reach() {
    on_lavapipe(
        "render::frame_tests::a_camera_far_from_the_planes_draws_them_or_refuses_a_view_too_small_for_their_reach",
        |r| {
            // The origin planes, the view 100 km off along x (onto (100000, 10, 7), from −y):
            // their corners reach about 100,004 mm from the target, so the least view at 48
            // rows is 48 × 100,004 / 2^19 = 9.16 mm. At 44 they're off the view, drawn as
            // nothing; at 5 the frame is refused, naming the planes, though without them it's
            // drawn.
            let at = |view_height: f64| Camera {
                projection: Projection::Orthographic { view_height },
                eye: [100_000.0, -50.0, 7.0],
                target: [100_000.0, 10.0, 7.0],
                up: [0.0, 0.0, 1.0],
            };
            let planes = origin_planes(None);
            let camera = at(44.0);
            let without = image(r, &frame(&camera, &[], &[], None));
            let with = image(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[], &[], None)
                },
            );
            assert!(with == without);
            let small = at(5.0);
            draw(r, &frame(&small, &[], &[], None));
            assert_eq!(
                refused(
                    r,
                    &Frame {
                        planes: &planes,
                        ..frame(&small, &[], &[], None)
                    }
                ),
                "planes"
            );
            // A perspective eye as far: drawn, the planes a speck at the view's centre.
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 40.0,
                },
                eye: [21.0, -1.0e6, 21.0],
                target: [21.0, 0.0, 21.0],
                up: [0.0, 0.0, 1.0],
            };
            draw(
                r,
                &Frame {
                    planes: &planes,
                    ..frame(&camera, &[], &[], None)
                },
            );
        },
    );
}

// The council's round 1 on #507.

/// [`facing`], its sides swapped: v × u, so it faces away from [`front`]'s eye (+y), where
/// [`facing`]'s u × v faces it (−y).
fn facing_away(y: f64, fill: [u8; 4], outline: [u8; 4]) -> super::Plane {
    let plane = facing(y, fill, outline);
    super::Plane {
        u: plane.v,
        v: plane.u,
        ..plane
    }
}

#[test]
fn a_plane_is_filled_alike_from_either_side() {
    on_lavapipe(
        "render::frame_tests::a_plane_is_filled_alike_from_either_side",
        |r| {
            // The prototype's `DoubleSide`: the same plane seen from its back (its normal
            // away from the eye) and its front, over the background and over the box's front
            // face behind it, each pixel tinted as by hand.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            assert!(dot_toward_eye(&facing(-5.0, CLEAR, CLEAR)) > 0.0);
            assert!(dot_toward_eye(&facing_away(-5.0, CLEAR, CLEAR)) < 0.0);
            for bodies in [&[][..], &[1][..]] {
                let without = image(r, &frame(&camera, bodies, &[], None));
                for plane in [
                    facing(-5.0, HALF_MAGENTA, CLEAR),
                    facing_away(-5.0, HALF_MAGENTA, CLEAR),
                ] {
                    let planes = [plane];
                    let with = image(
                        r,
                        &Frame {
                            planes: &planes,
                            ..frame(&camera, bodies, &[], None)
                        },
                    );
                    for (x, y) in [(22, 19), (32, 26), (24, 23), (42, 28)] {
                        let under = pixel(&without, x, y);
                        assert_colour(
                            pixel(&with, x, y),
                            over(under, HALF_MAGENTA),
                            3.0,
                            &format!("({x}, {y}) of {plane:?} over {bodies:?}"),
                        );
                    }
                }
            }
        },
    );
}

/// Which way `plane`'s u × v points against [`front`]'s view (+y, from the eye): above 0
/// toward the eye.
fn dot_toward_eye(plane: &super::Plane) -> f64 {
    let n = silico_protocol::vec3::cross(plane.u, plane.v);
    -n[1]
}

#[test]
fn a_translucent_outline_is_blended_over_the_background_and_a_face_as_by_hand() {
    on_lavapipe(
        "render::frame_tests::a_translucent_outline_is_blended_over_the_background_and_a_face_as_by_hand",
        |r| {
            // Green at alpha 128, 3 points wide, its fill clear: its left side through the
            // centre of column 20 covers that column whole (a stroke's coverage is 1 within
            // its half width less half a pixel), each pixel green over what's there as by
            // hand, `(1 − a) · under + a · green` in linear light: over the background, and,
            // the plane in front of the box, over its front face.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            const HALF_GREEN: [u8; 4] = [0, 255, 0, 128];
            let plane = super::Plane {
                width: 3.0,
                ..facing(-5.0, CLEAR, HALF_GREEN)
            };
            let planes = [plane];
            for bodies in [&[][..], &[1][..]] {
                let without = image(r, &frame(&camera, bodies, &[], None));
                let with = image(
                    r,
                    &Frame {
                        planes: &planes,
                        ..frame(&camera, bodies, &[], None)
                    },
                );
                for y in 23..=28 {
                    let under = pixel(&without, 20, y);
                    assert_colour(
                        pixel(&with, 20, y),
                        over(under, HALF_GREEN),
                        3.0,
                        &format!("(20, {y}) over {under:?}, bodies {bodies:?}"),
                    );
                }
                if bodies.is_empty() {
                    assert_eq!(pixel(&without, 20, 25), BLACK);
                } else {
                    assert!(!within(pixel(&without, 20, 25), BLACK, 8), "the face");
                }
            }
        },
    );
}

#[test]
fn a_plane_off_the_view_is_logged_but_not_drawn() {
    on_lavapipe(
        "render::frame_tests::a_plane_off_the_view_is_logged_but_not_drawn",
        |r| {
            // Two planes: one before the box, and one 300 mm off along x (the view is 58.67 mm
            // wide about x = 21), which isn't drawn. Logged once: 2 planes, 1 drawn. A frame of
            // none logs none.
            r.upload(1, &the_box()).unwrap();
            let camera = front();
            let seen = facing(-5.0, HALF_MAGENTA, GREEN);
            let off = super::Plane {
                origin: [300.0, -5.0, 0.0],
                ..seen
            };
            let planes = [seen, off];
            let events = logged("planes", ["planes", "drawn"], || {
                draw(
                    r,
                    &Frame {
                        planes: &planes,
                        ..frame(&camera, &[1], &[], None)
                    },
                );
            });
            assert_eq!(events, [(2, 1)]);
            let none = logged("planes", ["planes", "drawn"], || {
                draw(r, &frame(&camera, &[1], &[], None));
            });
            assert!(none.is_empty(), "{none:?}");
        },
    );
}

#[test]
fn a_planes_fill_is_uploaded_once_for_its_colour_and_placed_for_every_frame() {
    on_lavapipe(
        "render::frame_tests::a_planes_fill_is_uploaded_once_for_its_colour_and_placed_for_every_frame",
        |r| {
            // The three origin planes, plain, then one lit: a mesh for each colour, the same
            // one frame after frame from every camera, and the bodies' count untouched.
            r.upload(1, &the_box()).unwrap();
            let plain = origin_planes(None);
            let cameras = [
                front(),
                front_in_perspective(),
                from_the_corner(Projection::Orthographic { view_height: 60.0 }),
            ];
            draw(
                r,
                &Frame {
                    planes: &plain,
                    ..frame(&cameras[0], &[1], &[], None)
                },
            );
            assert_eq!(r.plane_meshes.len(), 1);
            let made = Arc::clone(r.plane_meshes.values().next().unwrap());
            let bodies = r.uploads();
            for camera in &cameras {
                for _ in 0..3 {
                    draw(
                        r,
                        &Frame {
                            planes: &plain,
                            ..frame(camera, &[1], &[], None)
                        },
                    );
                }
            }
            assert_eq!(r.plane_meshes.len(), 1);
            assert!(Arc::ptr_eq(r.plane_meshes.values().next().unwrap(), &made));
            let lit = origin_planes(Some(2));
            draw(
                r,
                &Frame {
                    planes: &lit,
                    ..frame(&cameras[2], &[1], &[], None)
                },
            );
            assert_eq!(r.plane_meshes.len(), 2);
            assert_eq!(r.uploads(), bodies);
            // Past the most colours kept, those kept let go and the colours drawn made
            // again: never more than the most.
            for alpha in 0..=u8::try_from(super::planes::MAX_FILL_MESHES).unwrap() {
                let tinted = [super::Plane {
                    fill: [10, 20, 30, alpha + 1],
                    ..plain[0]
                }];
                draw(
                    r,
                    &Frame {
                        planes: &tinted,
                        ..frame(&cameras[0], &[1], &[], None)
                    },
                );
                assert!(r.plane_meshes.len() <= super::planes::MAX_FILL_MESHES);
            }
        },
    );
}
