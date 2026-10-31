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

use silico_protocol::{Camera, Mesh, Projection};

use super::tests::on_lavapipe;
use re_renderer::view_builder::{OrthographicCameraMode, Projection as ViewProjection};

use super::{
    BACKGROUND, Element, Frame, MAX_PENDING_PICKS, MAX_PICKING_SIDE, OUTPUT_FORMAT, PendingPick,
    Pick, Picking, PixelRect, RenderFailure, Renderer,
};
use crate::bench::{box_mesh, front_of_a_box};

const WIDTH: u32 = 64;
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
        camera,
        size_px: [WIDTH, HEIGHT],
        pixels_per_point: 1.0,
        bodies,
        highlights,
        picking,
    }
}

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
            picking: None,
            ..*frame
        },
    );
    r.take_picking();
    let asked = frame.picking.unwrap();
    draw(r, frame);
    let serial = r.frame();
    assert_eq!(r.take_picking(), None, "answered in its own frame");
    draw(
        r,
        &Frame {
            picking: None,
            ..*frame
        },
    );
    let picking = r.take_picking().expect("not answered a frame later");
    assert_eq!((picking.frame, picking.rect), (serial, asked));
    assert_eq!(r.frame(), serial + 1);
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
            // box moved 100 mm right (out of view: from column 32 + 79 × 12/11 = 118), is
            // uploaded again and drawn where it now is.
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
            assert_eq!(r.uploads(), before + 6);
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
    let device = r.ctx.device.clone();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("composite target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    // 64 pixels of 4 bytes: 256 bytes a row, as a copy asks.
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("composite readback"),
        size: u64::from(WIDTH * HEIGHT * 4),
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
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
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
    data.chunks_exact(4)
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
            // The corner is the background, opaque; the centre is the front face, not it.
            let [red, green, blue] = BACKGROUND;
            let corner = at(1, 1);
            for (got, want) in corner.iter().zip([red, green, blue, 255]) {
                assert!(got.abs_diff(want) <= 2, "{corner:?}");
            }
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
                draw(r, &frame(&camera, bodies, &[], None));
                r.take_picking().expect("no answer")
            };
            r.upload(1, &the_box()).unwrap();
            // Asked of the box, then its mesh replaced (moved 1 mm: still under the
            // rectangle, so only the generation tells): the answer comes, naming nothing.
            draw(r, &one);
            let asked = r.frame();
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
            // once, then none: an outline, never a tinted mesh.
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
                    .all(|f| f.tinted.is_none() && f.plain.is_some())
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
