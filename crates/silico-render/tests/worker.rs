//! The render worker, driven as the core drives it (`silico_core::worker::Worker`, its
//! environment cleared): a box's ID buffer and PNG read against its projection computed
//! by hand, never against a stored image (docs/design/m4.6-render.md, Tests; R5).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Blob, Camera, EDGE_BIT, Generation, Mesh, Op, PixelId, Projection, RenderBody, RenderScene,
    Reply, ShapeData,
};
use silico_render::bench::{box_mesh, front_of_a_box};
use silico_render::render::{BACKGROUND, BODY, HIGHLIGHT};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-render-worker");
const TIMEOUT: Duration = Duration::from_secs(30);

fn worker() -> Worker {
    Worker::spawn_as(Path::new(WORKER), WorkerKind::Render).unwrap()
}

fn body(number: u32, mesh: Mesh) -> RenderBody {
    RenderBody {
        body: number,
        mesh,
        highlight_faces: vec![],
        highlight_edges: vec![],
    }
}

/// The bench's box from the front, `width` by `height`, its body or bodies `bodies`.
fn front(width: u32, height: u32, bodies: Vec<RenderBody>) -> RenderScene {
    RenderScene {
        bodies,
        ..front_of_a_box(width, height)
    }
}

/// The box's front face, by hand about the view's centre: x 0 to 40 and z 0 to 10 about the
/// target (21, 10, 7).
const FRONT_FACE_MM: [f64; 4] = [-21.0, 19.0, -7.0, 3.0];

/// An image: its size and RGB pixels, row by row from the top left.
struct Image {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Image {
    fn at(&self, col: u32, row: u32) -> [u8; 3] {
        let i = usize::try_from((row * self.width + col) * 3).unwrap();
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

/// The PNG decoded, which must be 8-bit RGB.
fn decode(png: &[u8]) -> Image {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().unwrap();
    let mut rgb = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut rgb).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgb, png::BitDepth::Eight)
    );
    rgb.truncate(info.buffer_size());
    Image {
        width: info.width,
        height: info.height,
        rgb,
    }
}

fn render(w: &mut Worker, scene: RenderScene) -> (Image, Option<Vec<PixelId>>) {
    match w
        .call(Generation(1), Op::Render(Box::new(scene)), TIMEOUT)
        .unwrap()
    {
        Reply::Rendered { png, ids } => (decode(&png), ids),
        other => panic!("Render replied {other:?}"),
    }
}

/// Where a rectangle in the view lands, in pixels: x from `left` to `right` and z from
/// `bottom` to `top`, in mm about the view's centre, `pixel` pixels a mm.
struct Projected {
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
}

fn projected(
    width: u32,
    height: u32,
    view_height: f64,
    [left, right, bottom, top]: [f64; 4],
) -> Projected {
    let pixel = f64::from(height) / view_height;
    let (cx, cy) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
    Projected {
        x0: cx + left * pixel,
        x1: cx + right * pixel,
        y0: cy - top * pixel,
        y1: cy - bottom * pixel,
    }
}

impl Projected {
    /// Whether a pixel's centre is more than `margin` pixels inside, or outside.
    fn inside(&self, col: u32, row: u32, margin: f64) -> bool {
        let (x, y) = (f64::from(col) + 0.5, f64::from(row) + 0.5);
        x > self.x0 + margin && x < self.x1 - margin && y > self.y0 + margin && y < self.y1 - margin
    }

    fn outside(&self, col: u32, row: u32, margin: f64) -> bool {
        let (x, y) = (f64::from(col) + 0.5, f64::from(row) + 0.5);
        x < self.x0 - margin || x > self.x1 + margin || y < self.y0 - margin || y > self.y1 + margin
    }
}

const FRONT_FACE: PixelId = [1, 3];
const FRONT_EDGES: [PixelId; 4] = [
    [1, EDGE_BIT | 1],
    [1, EDGE_BIT | 2],
    [1, EDGE_BIT | 3],
    [1, EDGE_BIT | 4],
];

#[test]
fn a_box_from_the_front_is_its_front_face_over_its_projection_its_edges_on_its_boundary() {
    let mut w = worker();
    // The smallest; wider than high, so the view's aspect is the image's, and 401 wide, so
    // the readback's rows are padded; and the largest.
    for (width, height) in [(1, 1), (401, 300), (4096, 4096)] {
        let (image, ids) = render(&mut w, front_of_a_box(width, height));
        let ids = ids.unwrap();
        assert_eq!((image.width, image.height), (width, height));
        assert_eq!(ids.len(), usize::try_from(width * height).unwrap());
        let face = projected(width, height, 44.0, FRONT_FACE_MM);
        let mut seen_edges = std::collections::BTreeSet::new();
        for row in 0..height {
            for col in 0..width {
                let id = ids[usize::try_from(row * width + col).unwrap()];
                if face.inside(col, row, 1.0) {
                    assert_eq!(
                        id, FRONT_FACE,
                        "{width}: ({col}, {row}) inside the front face"
                    );
                } else if face.outside(col, row, 1.0) {
                    assert_eq!(id, [0, 0], "{width}: ({col}, {row}) outside the box");
                } else {
                    assert!(
                        id == FRONT_FACE || id == [0, 0] || FRONT_EDGES.contains(&id),
                        "{width}: ({col}, {row}) on the front face's boundary is {id:?}"
                    );
                }
                if FRONT_EDGES.contains(&id) {
                    seen_edges.insert(id);
                }
            }
        }
        if width == 1 {
            // A pixel's centre over the face, near its top right.
            assert!(
                ids[0] == FRONT_FACE || FRONT_EDGES.contains(&ids[0]),
                "{ids:?}"
            );
            continue;
        }
        assert_eq!(
            seen_edges.len(),
            4,
            "{width}: the front face's edges seen: {seen_edges:?}"
        );
        // The image: the background where the box isn't, and the body's colour, lit straight
        // on, inside the face.
        assert_eq!(image.at(0, 0), BACKGROUND);
        assert_eq!(image.at(width - 1, height - 1), BACKGROUND);
        let inside = image.at(
            ((face.x0 + face.x1) / 2.0) as u32,
            ((face.y0 + face.y1) / 2.0) as u32,
        );
        for k in 0..3 {
            assert!(
                inside[k].abs_diff(BODY[k]) <= 1,
                "{width}: inside the face {inside:?}"
            );
        }
    }
}

#[test]
fn a_highlighted_face_is_bluer_over_it_and_nothing_away_from_it_changes() {
    let mut w = worker();
    let (width, height) = (400, 300);
    let plain = front(
        width,
        height,
        vec![body(1, box_mesh([0.0; 3], [40.0, 20.0, 10.0]))],
    );
    let mut lit = plain.clone();
    lit.bodies[0].highlight_faces = vec![2];
    let (before, before_ids) = render(&mut w, plain);
    let (after, after_ids) = render(&mut w, lit);
    // The ID buffer marks no highlight.
    assert_eq!(before_ids, after_ids);
    let face = projected(width, height, 44.0, FRONT_FACE_MM);
    for row in 0..height {
        for col in 0..width {
            let (b, a) = (before.at(col, row), after.at(col, row));
            if face.inside(col, row, 2.0) {
                assert!(
                    a[2] > b[2] && a[0] < b[0] && a[1] < b[1],
                    "({col}, {row}): {b:?} to {a:?}"
                );
            } else if face.outside(col, row, 4.0) {
                assert_eq!(a, b, "({col}, {row}) away from the face");
            }
        }
    }
    // Half the highlight's blue, by hand in linear light, at the centre: the face is lit
    // straight on, so its colour there is the body's.
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            libm::pow((c + 0.055) / 1.055, 2.4)
        }
    };
    let srgb = |l: f64| {
        let c = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * libm::pow(l, 1.0 / 2.4) - 0.055
        };
        (c * 255.0).round()
    };
    let centre = after.at(
        ((face.x0 + face.x1) / 2.0) as u32,
        ((face.y0 + face.y1) / 2.0) as u32,
    );
    for k in 0..3 {
        let want = srgb(0.5 * linear(BODY[k]) + 0.5 * linear(HIGHLIGHT[k]));
        assert!(
            (f64::from(centre[k]) - want).abs() <= 1.0,
            "the centre {centre:?}, want {want} in channel {k}"
        );
    }
}

#[test]
fn a_highlighted_edge_is_bluer_on_it() {
    let mut w = worker();
    let (width, height) = (400, 300);
    let plain = front(
        width,
        height,
        vec![body(1, box_mesh([0.0; 3], [40.0, 20.0, 10.0]))],
    );
    let mut lit = plain.clone();
    // Edge1, the front face's bottom.
    lit.bodies[0].highlight_edges = vec![0];
    let (before, ids) = render(&mut w, plain);
    let (after, _) = render(&mut w, lit);
    let ids = ids.unwrap();
    let mut on_it = 0;
    for row in 0..height {
        for col in 0..width {
            if ids[usize::try_from(row * width + col).unwrap()] == [1, EDGE_BIT | 1] {
                let (b, a) = (before.at(col, row), after.at(col, row));
                assert!(a[2] > b[2], "({col}, {row}) on Edge1: {b:?} to {a:?}");
                on_it += 1;
            }
        }
    }
    assert!(on_it > 0);
}

#[test]
fn bodies_carry_their_numbers_and_a_far_part_draws_as_one_near_the_origin() {
    let mut w = worker();
    let (width, height) = (300, 300);
    let scene = |offset: [f64; 3]| {
        let mut scene = front(
            width,
            height,
            vec![
                body(
                    3,
                    box_mesh([offset[0] - 30.0, offset[1], offset[2]], [20.0, 20.0, 10.0]),
                ),
                body(
                    7,
                    box_mesh([offset[0] + 30.0, offset[1], offset[2]], [20.0, 20.0, 10.0]),
                ),
            ],
        );
        scene.camera.projection = Projection::Orthographic { view_height: 100.0 };
        scene.camera.eye = [offset[0] + 10.0, offset[1] - 50.0, offset[2] + 5.0];
        scene.camera.target = [offset[0] + 10.0, offset[1] + 10.0, offset[2] + 5.0];
        scene
    };
    let (_, near) = render(&mut w, scene([0.0; 3]));
    let near = near.unwrap();
    // By hand: body 3's front face is x -40 to -20 about the target, body 7's 20 to 40.
    for (number, x) in [(3, -30.0), (7, 30.0)] {
        let col = (f64::from(width) / 2.0 + x * f64::from(height) / 100.0) as u32;
        assert_eq!(
            near[usize::try_from(height / 2 * width + col).unwrap()],
            [number, 3]
        );
    }
    // 50 km from the origin, drawn about the target: the same ID buffer.
    let (_, far) = render(&mut w, scene([5e4, -3e4, 2e4]));
    assert!(far.unwrap() == near, "the far part's ID buffer differs");
}

#[test]
fn a_perspective_view_puts_the_front_faces_centre_at_the_images_centre() {
    let mut w = worker();
    let mut scene = front(
        200,
        200,
        vec![body(1, box_mesh([0.0; 3], [40.0, 20.0, 10.0]))],
    );
    scene.camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [20.0, -80.0, 5.0],
        target: [20.0, 0.0, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    let (_, ids) = render(&mut w, scene);
    let ids = ids.unwrap();
    assert_eq!(ids[100 * 200 + 100], FRONT_FACE);
    assert_eq!(ids[0], [0, 0]);
}

#[test]
fn without_ids_asked_for_there_are_none() {
    let mut w = worker();
    let mut scene = front(
        64,
        48,
        vec![body(1, box_mesh([0.0; 3], [40.0, 20.0, 10.0]))],
    );
    scene.ids = false;
    let (image, ids) = render(&mut w, scene);
    assert!(ids.is_none());
    assert_eq!((image.width, image.height), (64, 48));
}

#[test]
fn a_scene_that_fails_its_check_is_refused_naming_the_field_and_nothing_else_is_this_workers() {
    let mut w = worker();
    let mut scene = front(0, 48, vec![]);
    let e = w
        .call(Generation(1), Op::Render(Box::new(scene.clone())), TIMEOUT)
        .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert!(e.message().contains("width 0"), "{e:?}");
    scene.width = 64;
    scene.bodies = vec![body(1, box_mesh([0.0; 3], [1.0; 3]))];
    scene.bodies[0].mesh.triangles[0] = [0, 1, 24];
    let e = w
        .call(Generation(1), Op::Render(Box::new(scene)), TIMEOUT)
        .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert!(
        e.message()
            .contains("triangle 0 indexes past its 24 positions"),
        "{e:?}"
    );
    // The kernel's operations are refused; the worker still answers after each.
    let e = w
        .call(
            Generation(1),
            Op::Tessellate {
                shape: ShapeData::checked(Blob::inline(vec![1]), true),
                deflection: 0.1,
            },
            TIMEOUT,
        )
        .unwrap_err();
    assert_eq!(e.code(), ErrorCode::Internal, "{e}");
    assert_eq!(e.message(), "the render worker couldn't do Tessellate");
    assert_eq!(
        e.cause(),
        Some("Tessellate is answered by another kind of worker, not this one")
    );
    match w
        .call(
            Generation(1),
            Op::Echo {
                data: Blob::inline(vec![5]),
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Echo { data } => assert_eq!(data, Blob::inline(vec![5])),
        other => panic!("Echo replied {other:?}"),
    }
}
