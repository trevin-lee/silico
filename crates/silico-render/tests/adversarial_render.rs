//! Hostile scenes for the render worker (`/break-it`, docs/design/m4.6-render.md, Tests:
//! "Hostile scenes"), driven as the core drives it. Each scene is refused, naming its field,
//! or drawn, and what's drawn is checked against a projection computed by hand here, never
//! against a stored image. After every refusal the same worker must still draw.
//!
//! Tests marked `Was a break:` each found a bug the break-it run reported, since fixed;
//! they guard against its return.

use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use silico_core::worker::{Worker, WorkerKind};
use silico_core::{Error, ErrorCode};
use silico_protocol::{
    Camera, EDGE_BIT, Generation, MAX_RENDER_EDGES, MAX_RENDER_FACES, Mesh, Op, PixelId,
    Projection, RenderBody, RenderScene, Reply,
};
use silico_render::bench::{box_mesh, front_of_a_box};
use silico_render::render::{AMBIENT, BACKGROUND, BODY};

use crate::common::WAIT;
use crate::share::Share;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-render-worker");
/// The core's render limit (`RENDER_TIMEOUT`): what the scale tests bound a draw's cost by.
const LIMIT: Duration = Duration::from_secs(30);

const FRONT_FACE: PixelId = [1, 3];

/// A way to spoil a mesh or a camera, and what its refusal says.
type MeshCase = (fn(&mut Mesh), &'static str);
type CameraCase = (fn(&mut Camera), &'static str);

fn worker() -> Worker {
    Worker::spawn_as(Path::new(WORKER), WorkerKind::Render).unwrap()
}

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

type Drawn = (Image, Option<Vec<PixelId>>);

/// `scene` drawn by `w`, or its refusal, allowed `WAIT`: no caller claims a time by it.
fn call(w: &mut Worker, scene: &RenderScene) -> Result<Drawn, Error> {
    match w.call(Generation(1), Op::Render(Box::new(scene.clone())), WAIT)? {
        Reply::Rendered { png, ids } => Ok((decode(&png), ids)),
        other => panic!("Render replied {other:?}"),
    }
}

/// The ID buffer of `scene`, which must be drawn, at the size asked.
fn ids(w: &mut Worker, scene: &RenderScene) -> Vec<PixelId> {
    let (image, ids) = call(w, scene).unwrap_or_else(|e| panic!("refused: {e:?}"));
    assert_eq!((image.width, image.height), (scene.width, scene.height));
    let ids = ids.unwrap();
    assert_eq!(
        ids.len() as u64,
        u64::from(scene.width) * u64::from(scene.height)
    );
    ids
}

/// The pixel of the front face's centre in `front_of_a_box(width, height)`: the face is x
/// −21 to 19 mm and z −7 to 3 mm about the view's centre, 44 mm high.
fn front_centre(width: u32, height: u32) -> usize {
    let pixel = f64::from(height) / 44.0;
    let col = (f64::from(width) / 2.0 - 1.0 * pixel) as u32;
    let row = (f64::from(height) / 2.0 + 2.0 * pixel) as u32;
    usize::try_from(row * width + col).unwrap()
}

/// The worker still draws the bench's box after whatever came before.
fn still_draws(w: &mut Worker) {
    let ids = ids(w, &front_of_a_box(64, 48));
    assert_eq!(
        ids[front_centre(64, 48)],
        FRONT_FACE,
        "the box after a refusal"
    );
}

/// `scene` refused as `invalid_input`, naming `parameter`, with `words` in its message; and
/// the worker still draws after it.
fn refused(w: &mut Worker, scene: &RenderScene, parameter: &str, words: &str) {
    let e = match call(w, scene) {
        Ok(_) => panic!("drawn, not refused: {words}"),
        Err(e) => e,
    };
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert_eq!(e.parameter(), Some(parameter), "{e:?}");
    assert!(e.message().contains(words), "{e:?}");
    still_draws(w);
}

fn body(number: u32, mesh: Mesh) -> RenderBody {
    RenderBody {
        body: number,
        mesh,
        highlight_faces: vec![],
        highlight_edges: vec![],
    }
}

fn the_box() -> Mesh {
    box_mesh([0.0; 3], [40.0, 20.0, 10.0])
}

fn with_mesh(width: u32, height: u32, mesh: Mesh) -> RenderScene {
    RenderScene {
        bodies: vec![body(1, mesh)],
        ..front_of_a_box(width, height)
    }
}

/// Where a point lands in the image, by hand, in f64: the camera's basis (forward from the
/// eye to the target, right = forward × up, up = right × forward), and the projection.
struct Projector {
    eye: [f64; 3],
    target: [f64; 3],
    forward: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
    projection: Projection,
    width: f64,
    height: f64,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

impl Projector {
    fn of(scene: &RenderScene) -> Projector {
        let c = &scene.camera;
        let forward = unit(sub(c.target, c.eye));
        let right = unit(cross(forward, c.up));
        let up = cross(right, forward);
        Projector {
            eye: c.eye,
            target: c.target,
            forward,
            right,
            up,
            projection: c.projection,
            width: f64::from(scene.width),
            height: f64::from(scene.height),
        }
    }

    /// The point's place in pixels (column, row), pixel centres at half-integers.
    fn pixel(&self, p: [f64; 3]) -> (f64, f64) {
        let (x, y, scale) = match self.projection {
            Projection::Orthographic { view_height } => {
                let d = sub(p, self.target);
                (
                    dot(d, self.right),
                    dot(d, self.up),
                    self.height / view_height,
                )
            }
            Projection::Perspective { field_of_view } => {
                let d = sub(p, self.eye);
                let depth = dot(d, self.forward);
                let tan = libm::tan(field_of_view.to_radians() / 2.0);
                (
                    dot(d, self.right) / depth,
                    dot(d, self.up) / depth,
                    self.height / 2.0 / tan,
                )
            }
        };
        (self.width / 2.0 + x * scale, self.height / 2.0 - y * scale)
    }
}

/// Pixels where `got` differs from `want`, leaving out those within `margin` pixels of a
/// boundary in `want` (a rasterizer's quantization, which the vertex transform's f32 math
/// can move by a subpixel).
fn differ_away_from_boundaries(
    want: &[PixelId],
    got: &[PixelId],
    width: u32,
    height: u32,
    margin: i64,
) -> Vec<(u32, u32, PixelId, PixelId)> {
    let (w, h) = (i64::from(width), i64::from(height));
    let at = |c: i64, r: i64| want[usize::try_from(r * w + c).unwrap()];
    let mut out = Vec::new();
    for r in 0..h {
        for c in 0..w {
            let i = usize::try_from(r * w + c).unwrap();
            if got[i] == want[i] {
                continue;
            }
            let uniform = (-margin..=margin).all(|dr| {
                (-margin..=margin).all(|dc| {
                    let (rr, cc) = (r + dr, c + dc);
                    rr < 0 || cc < 0 || rr >= h || cc >= w || at(cc, rr) == want[i]
                })
            });
            if uniform {
                out.push((c as u32, r as u32, want[i], got[i]));
            }
        }
    }
    out
}

fn assert_same_away_from_boundaries(want: &[PixelId], got: &[PixelId], w: u32, h: u32, what: &str) {
    let off = differ_away_from_boundaries(want, got, w, h, 1);
    assert!(
        off.is_empty(),
        "{what}: {} pixels away from any boundary differ, the first {:?}",
        off.len(),
        &off[..off.len().min(5)]
    );
}

// ---------------------------------------------------------------------------------------
// Refused: the check, in the worker, and the worker still draws after each.

#[test]
fn a_triangle_indexing_u32_max_is_refused_and_the_worker_still_draws() {
    let mut w = worker();
    let mut mesh = the_box();
    mesh.triangles[5] = [0, u32::MAX, 1];
    refused(
        &mut w,
        &with_mesh(32, 32, mesh),
        "bodies",
        "triangle 5 indexes past its 24 positions",
    );
}

#[test]
fn non_finite_positions_normals_and_edge_points_are_refused_each_naming_which() {
    let mut w = worker();
    let cases: [MeshCase; 6] = [
        (
            |m| m.positions[3][1] = f64::NAN,
            "position 3 must be finite",
        ),
        (
            |m| m.positions[0][0] = f64::INFINITY,
            "position 0 must be finite",
        ),
        (
            |m| m.positions[23][2] = f64::NEG_INFINITY,
            "position 23 must be finite",
        ),
        (|m| m.normals[7][0] = f32::NAN, "normal 7 isn't finite"),
        (|m| m.normals[0][2] = f32::INFINITY, "normal 0 isn't finite"),
        (
            |m| m.edge_points[23][0] = f64::NAN,
            "edge point 23 must be finite",
        ),
    ];
    for (spoil, words) in cases {
        let mut mesh = the_box();
        spoil(&mut mesh);
        refused(&mut w, &with_mesh(32, 32, mesh), "bodies", words);
    }
}

#[test]
fn counts_whose_sum_overflows_u32_are_refused_not_wrapped() {
    let mut w = worker();
    // Two counts of u32::MAX sum to 2 in u32 arithmetic, wrapped: the box's last two faces
    // would then index past the triangles.
    let mut mesh = the_box();
    mesh.face_triangles = vec![u32::MAX, 3];
    refused(
        &mut w,
        &with_mesh(32, 32, mesh),
        "bodies",
        "its faces count 4294967298 triangles, and it has 12",
    );
    let mut mesh = the_box();
    mesh.edge_point_counts = vec![u32::MAX, 25];
    refused(
        &mut w,
        &with_mesh(32, 32, mesh),
        "bodies",
        "its edges count 4294967320 points, and it has 24",
    );
}

#[test]
fn highlights_past_the_mesh_repeated_or_at_u32_max_are_refused() {
    let mut w = worker();
    for (faces, edges) in [
        (vec![6], vec![]),
        (vec![u32::MAX], vec![]),
        (vec![2, 2], vec![]),
        (vec![], vec![12]),
        (vec![], vec![3, 1]),
    ] {
        let mut scene = front_of_a_box(32, 32);
        scene.bodies[0].highlight_faces = faces;
        scene.bodies[0].highlight_edges = edges;
        refused(&mut w, &scene, "bodies", "must be ascending, each once");
    }
}

#[test]
fn a_camera_of_nan_or_infinite_numbers_is_refused_naming_its_field() {
    let mut w = worker();
    let cases: [CameraCase; 5] = [
        (|c| c.eye[0] = f64::NAN, "camera.eye"),
        (|c| c.target[2] = f64::INFINITY, "camera.target"),
        (|c| c.up = [0.0; 3], "camera.up"),
        (
            |c| {
                c.projection = Projection::Orthographic {
                    view_height: f64::NAN,
                }
            },
            "camera.view_height",
        ),
        (
            |c| {
                c.projection = Projection::Perspective {
                    field_of_view: f64::NAN,
                }
            },
            "camera.field_of_view",
        ),
    ];
    for (spoil, parameter) in cases {
        let mut scene = front_of_a_box(32, 32);
        spoil(&mut scene.camera);
        refused(&mut w, &scene, parameter, parameter);
    }
}

// ---------------------------------------------------------------------------------------
// Drawn: degenerate meshes, numbers at their ends.

#[test]
fn zero_area_triangles_cover_nothing_and_the_box_draws_as_before() {
    let mut w = worker();
    let mut mesh = the_box();
    // Face7: in front of the front face's centre (y = −1), three triangles of no area: one
    // vertex three times, three collinear points, and two vertices at one place.
    let base = u32::try_from(mesh.positions.len()).unwrap();
    mesh.positions.extend([
        [20.0, -1.0, 5.0],
        [10.0, -1.0, 5.0],
        [30.0, -1.0, 5.0],
        [20.0, -1.0, 5.0],
    ]);
    mesh.normals.extend([[0.0, -1.0, 0.0]; 4]);
    mesh.triangles.extend([
        [base, base, base],
        [base + 1, base, base + 2],
        [base, base + 3, base + 1],
    ]);
    mesh.face_triangles.push(3);
    let want = ids(&mut w, &front_of_a_box(200, 150));
    let got = ids(&mut w, &with_mesh(200, 150, mesh));
    assert!(!got.contains(&[1, 7]), "a face of no area covers pixels");
    assert_same_away_from_boundaries(&want, &got, 200, 150, "with Face7 of no area");
}

#[test]
fn faces_and_edges_with_nothing_to_draw_keep_the_numbers_of_those_after_them() {
    let mut w = worker();
    let mut mesh = the_box();
    // A face OCCT couldn't mesh (0 triangles) first, so the front face is Face4; an edge of
    // no points and one of one point first, so the front face's edges are Edge3 to Edge6.
    mesh.face_triangles.insert(0, 0);
    mesh.edge_points.insert(0, [20.0, -1.0, 5.0]);
    mesh.edge_point_counts.splice(0..0, [0, 1]);
    let (width, height) = (200, 150);
    let got = ids(&mut w, &with_mesh(width, height, mesh));
    assert_eq!(got[front_centre(width, height)], [1, 4]);
    let edges: std::collections::BTreeSet<u32> = got
        .iter()
        .filter(|id| id[1] & EDGE_BIT != 0)
        .map(|id| id[1] & !EDGE_BIT)
        .collect();
    assert_eq!(edges, [3, 4, 5, 6].into(), "the front face's edges");
    assert!(!got.contains(&[1, 3]) && !got.contains(&[1, 1]));
}

#[test]
fn a_zero_length_edge_draws_without_harm() {
    let mut w = worker();
    let mut mesh = the_box();
    // Edge13: two points at one place, on the front face's centre.
    mesh.edge_points.extend([[20.0, -0.0, 5.0]; 2]);
    mesh.edge_point_counts.push(2);
    let got = ids(&mut w, &with_mesh(200, 150, mesh));
    let centre = got[front_centre(200, 150)];
    assert!(
        centre == FRONT_FACE || centre == [1, EDGE_BIT | 13],
        "{centre:?}"
    );
    still_draws(&mut w);
}

#[test]
fn a_scene_of_no_bodies_and_a_body_of_no_mesh_are_the_background() {
    let mut w = worker();
    let empty = Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles: vec![],
        edge_points: vec![],
        edge_point_counts: vec![],
        valid: None,
    };
    for bodies in [vec![], vec![body(1, empty)]] {
        let scene = RenderScene {
            bodies,
            ..front_of_a_box(40, 30)
        };
        let (image, ids) = call(&mut w, &scene).unwrap();
        assert!(ids.unwrap().iter().all(|id| *id == [0, 0]));
        for row in 0..30 {
            for col in 0..40 {
                assert_eq!(image.at(col, row), BACKGROUND, "({col}, {row})");
            }
        }
    }
}

#[test]
fn a_body_numbered_u32_max_carries_its_number() {
    let mut w = worker();
    let scene = RenderScene {
        bodies: vec![body(u32::MAX, the_box())],
        ..front_of_a_box(64, 48)
    };
    let got = ids(&mut w, &scene);
    assert_eq!(got[front_centre(64, 48)], [u32::MAX, 3]);
}

#[test]
fn normals_of_no_length_inward_or_huge_light_by_lamberts_law_clamped() {
    // Was a break: the design shades in linear light (the albedo times 0.35 + 0.65
    // max(0, n·l)), but `albedo * light` scaled the `Rgba`'s alpha too, and `Color32::from`
    // then premultiplied in gamma: a face lit at the ambient only was (60, 61, 64), not
    // (105, 107, 111), and a highlighted face's tint off square was wrong the same way.
    let mut w = worker();
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
    // The front face's normal, by hand: none (ambient only), inward (ambient only), and far
    // longer than a unit toward the eye (Lambert's term clamped at 1, the body's colour).
    for (normal, light) in [
        ([0.0f32, 0.0, 0.0], f64::from(AMBIENT)),
        ([0.0, 1.0, 0.0], f64::from(AMBIENT)),
        ([0.0, -1e30, 0.0], 1.0),
    ] {
        let mut mesh = the_box();
        for n in &mut mesh.normals[8..12] {
            *n = normal;
        }
        let (width, height) = (64, 48);
        let (image, _) = call(&mut w, &with_mesh(width, height, mesh)).unwrap();
        let i = front_centre(width, height);
        let (col, row) = ((i % 64) as u32, (i / 64) as u32);
        let got = image.at(col, row);
        for k in 0..3 {
            let want = srgb(light * linear(BODY[k]));
            assert!(
                (f64::from(got[k]) - want).abs() <= 1.0,
                "normal {normal:?}: {got:?}, want {want} in channel {k}"
            );
        }
    }
}

#[test]
fn a_highlighted_face_and_edge_draw_at_every_extreme_size() {
    let mut w = worker();
    for (width, height) in [(1, 1), (1, 4096), (4096, 1), (2, 3), (3, 2), (4096, 2)] {
        let mut scene = front_of_a_box(width, height);
        scene.bodies[0].highlight_faces = vec![2];
        scene.bodies[0].highlight_edges = vec![0, 11];
        let (image, ids) =
            call(&mut w, &scene).unwrap_or_else(|e| panic!("{width} x {height}: {e:?}"));
        assert_eq!((image.width, image.height), (width, height));
        assert_eq!(
            ids.unwrap().len() as u64,
            u64::from(width) * u64::from(height)
        );
    }
    still_draws(&mut w);
}

// ---------------------------------------------------------------------------------------
// Floods: highlights, faces, edges, a part at the largest size.

/// `n` faces, each a square of two triangles, tiled over the front face's plane just in
/// front of it (y = −1), and an edge along each square's bottom; the box behind them.
fn tiles(n: u32) -> Mesh {
    let mut mesh = the_box();
    let side = f64::from(n).sqrt().ceil();
    let size = 40.0 / side;
    for k in 0..n {
        let (i, j) = (f64::from(k % side as u32), f64::from(k / side as u32));
        let (x, z) = (i * size, j * size * 0.25);
        let base = u32::try_from(mesh.positions.len()).unwrap();
        let s = size * 0.9;
        mesh.positions.extend([
            [x, -1.0, z],
            [x + s, -1.0, z],
            [x + s, -1.0, z + s * 0.25],
            [x, -1.0, z + s * 0.25],
        ]);
        mesh.normals.extend([[0.0, -1.0, 0.0]; 4]);
        mesh.triangles
            .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        mesh.face_triangles.push(2);
        mesh.edge_points.extend([[x, -1.0, z], [x + s, -1.0, z]]);
        mesh.edge_point_counts.push(2);
    }
    mesh
}

/// What a draw cost: its wall time over how much slower than a quiet machine this one ran
/// meanwhile (`Share`), which load doesn't move; with the two it's taken from, for a
/// failure to say.
#[derive(Clone, Copy)]
struct Cost {
    cost: Duration,
    wall: Duration,
    slowdown: f64,
}

impl std::fmt::Display for Cost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}: {:?} of wall time, the machine {:.2} times slower than a quiet one",
            self.cost, self.wall, self.slowdown
        )
    }
}

/// Draws `scene` once to make the context, then draws it again: what the second draw cost
/// ([`Cost`]), for a bound load doesn't move, and its result. The draw may take up to
/// `WAIT` of wall time: the bound is on its cost, which a loaded machine stretches.
fn timed(w: &mut Worker, scene: &RenderScene) -> (Cost, Result<Drawn, Error>) {
    still_draws(w);
    let share = Share::start();
    let start = Instant::now();
    let result = call(w, scene);
    let wall = start.elapsed();
    let slowdown = share.finish().slowdown;
    let cost = Cost {
        cost: wall.div_f64(slowdown),
        wall,
        slowdown,
    };
    (cost, result)
}

#[test]
fn the_most_edges_a_scene_may_have_draw_within_the_limit_and_one_more_is_refused() {
    let mut w = worker();
    let mut mesh = the_box();
    // MAX_RENDER_EDGES in all: the box's 12 and the rest short edges in a grid over the left
    // of the view, in front of the box, each highlighted.
    let n = u32::try_from(MAX_RENDER_EDGES).unwrap() - 12;
    for k in 0..n {
        let (i, j) = (f64::from(k % 1000), f64::from(k / 1000));
        let (x, z) = (-1.0 + i * 0.001, j * 0.01);
        mesh.edge_points
            .extend([[x, -1.0, z], [x + 0.0005, -1.0, z]]);
        mesh.edge_point_counts.push(2);
    }
    let mut scene = with_mesh(400, 300, mesh);
    scene.bodies[0].highlight_edges = (0..12 + n).collect();
    let (cost, result) = timed(&mut w, &scene);
    let (_, ids) = result.unwrap_or_else(|e| panic!("{cost}: {e:?}"));
    assert!(cost.cost < LIMIT, "{cost}");
    let ids = ids.unwrap();
    assert!(
        ids.iter().any(|id| id[1] > EDGE_BIT | 12),
        "no flood edge drawn"
    );
    assert_eq!(ids[front_centre(400, 300)], FRONT_FACE);
    // One more edge, in a body of its own: past the scene's limit, refused before drawing.
    let mut more = scene.clone();
    more.bodies.push(body(2, the_box()));
    more.bodies[1].mesh.edge_point_counts.truncate(1);
    more.bodies[1].mesh.edge_points.truncate(2);
    more.bodies[1].mesh.edge_point_counts[0] = 2;
    refused(
        &mut w,
        &more,
        "bodies",
        &format!(
            "the bodies have {} edges, more than the {MAX_RENDER_EDGES}",
            MAX_RENDER_EDGES + 1
        ),
    );
}

#[test]
fn ten_thousand_highlighted_faces_draw_within_half_the_limit() {
    // A large imported part has thousands of faces (NIST's CTC 02 has 487); each face is a
    // mesh and a draw of its own (the design's cost, M5's to meet), so 10,000 must still be
    // well within the limit.
    let mut w = worker();
    let n = 10_000;
    let mut scene = with_mesh(400, 300, tiles(n));
    scene.bodies[0].highlight_faces = (0..6 + n).collect();
    let (cost, result) = timed(&mut w, &scene);
    let (_, ids) = result.unwrap_or_else(|e| panic!("{cost}: {e:?}"));
    // Half the limit, as the 3,000 bodies' test (below) holds. Half still fails a cost
    // several times larger, and is what an agent's 30 s can count on.
    assert!(cost.cost < LIMIT / 2, "{cost}");
    let ids = ids.unwrap();
    // A tile's face, or (the tiles are under a pixel high) its edge.
    assert!(
        ids.iter().any(|id| (7..=6 + n).contains(&id[1])
            || (EDGE_BIT | 13..=EDGE_BIT | (12 + n)).contains(&id[1])),
        "no tile drawn"
    );
}

#[test]
fn the_most_faces_a_scene_may_have_draw_within_the_limit_and_one_more_is_refused() {
    // Each face is a mesh of its own, about 75 KiB of the worker's memory: MAX_RENDER_FACES
    // draw within the core's 30 s, and one more is refused before drawing, never killed.
    let mut w = worker();
    let n = u32::try_from(MAX_RENDER_FACES).unwrap() - 6;
    let mut scene = with_mesh(400, 300, tiles(n));
    scene.bodies[0].highlight_faces = (0..1000).collect();
    let (cost, result) = timed(&mut w, &scene);
    result.unwrap_or_else(|e| panic!("{cost}: {e:?}"));
    assert!(cost.cost < LIMIT, "{cost}");
    scene.bodies.push(body(2, tiles(1)));
    scene.bodies[1].mesh.face_triangles.truncate(1);
    let keep = scene.bodies[1].mesh.face_triangles[0] as usize;
    scene.bodies[1].mesh.triangles.truncate(keep);
    refused(
        &mut w,
        &scene,
        "bodies",
        &format!(
            "the bodies have {} faces, more than the {MAX_RENDER_FACES}",
            MAX_RENDER_FACES + 1
        ),
    );
}

/// A part of real size and density, made here: a 680 x 860 x 400 slab of `faces` curved
/// strips, each meshed by `per` triangles with smooth normals, and an edge of 40 points
/// along each strip's length, CTC 02's bounds and more than its 487 faces and 54,126
/// triangles at its floor.
fn ctc_scale(faces: u32, per: u32) -> Mesh {
    let mut mesh = Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles: vec![],
        edge_points: vec![],
        edge_point_counts: vec![],
        valid: None,
    };
    let columns = per / 2;
    for f in 0..faces {
        let x0 = -340.0 + 680.0 * f64::from(f) / f64::from(faces);
        let width = 680.0 / f64::from(faces);
        let base = u32::try_from(mesh.positions.len()).unwrap();
        for c in 0..=columns {
            let t = f64::from(c) / f64::from(columns);
            let z = -360.0 + 400.0 * t;
            // A ripple, so the strips' normals vary.
            let y = -40.0 + 5.0 * libm::sin(t * 20.0 + f64::from(f));
            for x in [x0, x0 + width] {
                mesh.positions.push([x, y, z]);
                let n = unit([0.0, -1.0, 0.25 * libm::cos(t * 20.0)]);
                mesh.normals.push([n[0] as f32, n[1] as f32, n[2] as f32]);
            }
            if c > 0 {
                let a = base + 2 * (c - 1);
                mesh.triangles
                    .extend([[a, a + 1, a + 3], [a, a + 3, a + 2]]);
            }
        }
        mesh.face_triangles.push(2 * columns);
        for k in 0..40 {
            let t = f64::from(k) / 39.0;
            mesh.edge_points.push([
                x0,
                -40.0 + 5.0 * libm::sin(t * 20.0 + f64::from(f)),
                -360.0 + 400.0 * t,
            ]);
        }
        mesh.edge_point_counts.push(40);
    }
    mesh
}

#[test]
fn a_part_of_ctc_size_at_4096_square_with_highlights_draws_within_the_limit() {
    let mut w = worker();
    let mesh = ctc_scale(2000, 300);
    let scene = RenderScene {
        bodies: vec![RenderBody {
            body: 1,
            mesh,
            highlight_faces: (0..1000).step_by(2).collect(),
            highlight_edges: (0..1000).step_by(2).skip(1).collect(),
        }],
        camera: Camera {
            projection: Projection::Orthographic { view_height: 450.0 },
            eye: [0.0, -1000.0, -160.0],
            target: [0.0, 0.0, -160.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 4096,
        height: 4096,
        ids: true,
    };
    let (cost, result) = timed(&mut w, &scene);
    let (image, ids) = result.unwrap_or_else(|e| panic!("{cost}: {e:?}"));
    assert!(cost.cost < LIMIT, "{cost}");
    assert_eq!((image.width, image.height), (4096, 4096));
    // The centre, by hand: x 0 is Face1001's left side, and the face 0.34 mm (3.1 pixels)
    // wide; column 2049's centre is 0.165 mm right of it.
    let ids = ids.unwrap();
    let id = ids[2048 * 4096 + 2049];
    assert!(id == [1, 1001] || id == [1, EDGE_BIT | 1001], "{id:?}");
}

// ---------------------------------------------------------------------------------------
// Cameras the check takes whose numbers don't survive the worker's f32.

/// The bench's box from the front, its target in front of the box (y = −10), so the eye can
/// be anywhere along the view's ray in front of it.
fn front_from(distance: f64, width: u32, height: u32) -> RenderScene {
    let mut scene = front_of_a_box(width, height);
    scene.camera.target = [21.0, -10.0, 7.0];
    scene.camera.eye = [21.0, -10.0 - distance, 7.0];
    scene
}

#[test]
fn an_up_too_long_for_f32_draws_as_a_unit_up() {
    // Was a break: 1e20 is finite, so the check took it, but its square overflowed f32 in
    // macaw's look_at (normalize_or_zero gave zero), which returned None, and the worker fell
    // back to the identity view (from the target down −z): a wrong picture as Ok. The frame
    // is now made in f64 (Camera::frame).
    let mut w = worker();
    let want = ids(&mut w, &front_from(60.0, 200, 150));
    let mut scene = front_from(60.0, 200, 150);
    scene.camera.up = [0.0, 0.0, 1e20];
    assert!(scene.check().is_ok());
    let got = ids(&mut w, &scene);
    assert_same_away_from_boundaries(&want, &got, 200, 150, "up (0, 0, 1e20)");
}

#[test]
fn an_up_too_short_for_f32_draws_as_a_unit_up() {
    // Was a break: as above, with an up of 1e-25: its square underflows f32 to 0.
    let mut w = worker();
    let want = ids(&mut w, &front_from(60.0, 200, 150));
    let mut scene = front_from(60.0, 200, 150);
    scene.camera.up = [0.0, 0.0, 1e-25];
    assert!(scene.check().is_ok());
    let got = ids(&mut w, &scene);
    assert_same_away_from_boundaries(&want, &got, 200, 150, "up (0, 0, 1e-25)");
}

#[test]
fn an_eye_nearer_the_target_than_a_nanometre_is_refused_and_one_a_nanometre_away_draws_the_same() {
    // Was a break: an eye 1e-30 from the target was apart from it, so the check took it; in
    // f32 the view's direction had a square of 0, look_at failed, and the identity view was
    // drawn. The box is moved so the target is the origin, where 1e-30 is representable.
    let mut w = worker();
    let scene = |distance: f64| RenderScene {
        bodies: vec![body(1, box_mesh([-21.0, 10.0, -7.0], [40.0, 20.0, 10.0]))],
        camera: Camera {
            projection: Projection::Orthographic { view_height: 44.0 },
            eye: [0.0, -distance, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        },
        width: 200,
        height: 150,
        ids: true,
    };
    // Now refused, naming the eye: a nanometre is the nearest it may be, which draws as
    // one 60 away does (an orthographic view uses only the eye's direction).
    let want = ids(&mut w, &scene(60.0));
    assert_eq!(want[front_centre(200, 150)], FRONT_FACE);
    refused(
        &mut w,
        &scene(1e-30),
        "camera.eye",
        "must be at least 0.000001 mm from camera.target",
    );
    let got = ids(&mut w, &scene(1e-6));
    assert_same_away_from_boundaries(&want, &got, 200, 150, "eye a nanometre from the target");
}

#[test]
fn an_orthographic_view_at_its_least_is_all_the_face_it_looks_at_and_smaller_is_refused() {
    // A view 1e-50 mm high is 0 in f32, and one under 1e-5 of how far the box reaches from
    // the target (21, −10, 7) (its farthest corner, (0, 20, 0), is sqrt(21² + 30² + 7²) =
    // 37.28 mm away, so 3.728e-4 at the least) would lose pixels past where f32 clips: each
    // refused,
    // naming it, rather than drawn wrong. At the least it may be, the view is inside the
    // front face (the target's ray meets it at (21, 0, 7)), so every pixel is the face.
    let mut w = worker();
    let mut scene = front_from(60.0, 40, 30);
    scene.camera.projection = Projection::Orthographic { view_height: 1e-50 };
    refused(
        &mut w,
        &scene,
        "camera.view_height",
        "camera.view_height 1e-50 must be from",
    );
    scene.camera.projection = Projection::Orthographic {
        view_height: 3.72e-4,
    };
    refused(
        &mut w,
        &scene,
        "camera.view_height",
        "is too small to draw the bodies in view",
    );
    scene.camera.projection = Projection::Orthographic {
        view_height: 3.73e-4,
    };
    let got = ids(&mut w, &scene);
    let off = got.iter().filter(|id| **id != FRONT_FACE).count();
    assert_eq!(off, 0, "pixels not the front face: {off} of {}", got.len());
}

#[test]
fn a_perspective_view_at_its_narrowest_is_all_the_face_it_looks_at_and_narrower_is_refused() {
    // As above, a field of view of 1e-40 degrees (under f32's smallest normal in radians),
    // refused, and 0.001, the narrowest it may be, all face.
    let mut w = worker();
    let mut scene = front_from(60.0, 40, 30);
    scene.camera.projection = Projection::Perspective {
        field_of_view: 1e-40,
    };
    refused(
        &mut w,
        &scene,
        "camera.field_of_view",
        "camera.field_of_view 1e-40 must be from 0.001 to 179.999",
    );
    scene.camera.projection = Projection::Perspective {
        field_of_view: 1e-3,
    };
    let got = ids(&mut w, &scene);
    let off = got.iter().filter(|id| **id != FRONT_FACE).count();
    assert_eq!(off, 0, "pixels not the front face: {off} of {}", got.len());
}

#[test]
fn a_field_of_view_at_its_widest_is_not_drawn_upside_down_and_wider_is_refused() {
    // 179.999999 degrees rounds past π in f32 radians, its half past π/2, and the tangent's
    // sign flips: refused, rather than drawn turned half a turn. 179.999, the widest it may
    // be, is drawn the right way up.
    //
    // By hand: from (0, −1, 0) toward +y, up +z (right +x), a wall at y = 0 over x and z
    // from 0 to 1e5 (a box 1 deep behind it) is the upper right quadrant: tan(89.9995°) is
    // 114591.56, so its far side is 0.8727 of the way to the image's edge.
    let mut w = worker();
    let scene = |fov: f64| RenderScene {
        bodies: vec![body(1, box_mesh([0.0; 3], [1e5, 1.0, 1e5]))],
        camera: Camera {
            projection: Projection::Perspective { field_of_view: fov },
            eye: [0.0, -1.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        },
        width: 100,
        height: 100,
        ids: true,
    };
    refused(
        &mut w,
        &scene(179.999_999),
        "camera.field_of_view",
        "camera.field_of_view 179.999999",
    );
    let scene = scene(179.999);
    let p = Projector::of(&scene);
    let (col, row) = p.pixel([1e5, 0.0, 1e5]);
    assert!(
        (col - 93.6).abs() < 0.1 && (row - 6.4).abs() < 0.1,
        "{col} {row}"
    );
    let got = ids(&mut w, &scene);
    let at = |c: usize, r: usize| got[r * 100 + c];
    assert_eq!(at(75, 25), FRONT_FACE, "the upper right quadrant");
    assert_eq!(at(25, 75), [0, 0], "the lower left quadrant");
    assert_eq!(at(25, 25), [0, 0], "the upper left quadrant");
    // Past the wall's far corner, no face: re_renderer sizes a line by the pixel at the
    // image's centre, so at so wide a view the wall's edges are drawn a few pixels wider
    // near the image's corner, but the face ends where it projects.
    assert_ne!(at(97, 3), FRONT_FACE, "past the wall's far corner");
    assert_ne!(at(99, 0), FRONT_FACE, "the image's far corner");
}

/// The bench's box from its top front right corner (the view from (1, −1, 1) toward its
/// centre), orthographic or perspective, `distance` from its centre along that ray, with
/// `up` given.
fn corner(distance: f64, up: [f64; 3], projection: Projection, size: u32) -> RenderScene {
    let target = [20.0, 10.0, 5.0];
    let d = distance / 3.0f64.sqrt();
    RenderScene {
        camera: Camera {
            projection,
            eye: [target[0] + d, target[1] - d, target[2] + d],
            target,
            up,
        },
        ..front_of_a_box(size, size)
    }
}

const ORTHO_50: Projection = Projection::Orthographic { view_height: 50.0 };

#[test]
fn a_box_from_a_corner_shows_exactly_its_three_near_faces_and_nine_near_edges_along_their_length() {
    let mut w = worker();
    let mesh = the_box();
    for projection in [
        ORTHO_50,
        Projection::Perspective {
            field_of_view: 40.0,
        },
    ] {
        let scene = corner(100.0, [0.0, 0.0, 1.0], projection, 300);
        let p = Projector::of(&scene);
        let got = ids(&mut w, &scene);
        let seen: std::collections::BTreeSet<PixelId> = got.iter().copied().collect();
        // Hidden: Face1 (x low), Face4 (y high), Face5 (z low), and the edges at the corner
        // (0, 20, 0): Edge5, Edge8, Edge9.
        for hidden in [
            [1, 1],
            [1, 4],
            [1, 5],
            [1, EDGE_BIT | 5],
            [1, EDGE_BIT | 8],
            [1, EDGE_BIT | 9],
        ] {
            assert!(!seen.contains(&hidden), "{projection:?}: {hidden:?} drawn");
        }
        for face in [2, 3, 6] {
            assert!(
                seen.contains(&[1, face]),
                "{projection:?}: Face{face} not drawn"
            );
        }
        // Each near edge's ID within a pixel and a half of its projection, at every tenth.
        for edge in [1u32, 2, 3, 4, 6, 7, 10, 11, 12] {
            let i = usize::try_from(2 * (edge - 1)).unwrap();
            let (a, b) = (mesh.edge_points[i], mesh.edge_points[i + 1]);
            let mut missing = vec![];
            for k in 1..10 {
                let t = f64::from(k) / 10.0;
                let q = [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ];
                let (col, row) = p.pixel(q);
                let found = (-1..=1).any(|dr: i64| {
                    (-1..=1).any(|dc: i64| {
                        let (c, r) = (
                            (col - 0.5).round() as i64 + dc,
                            (row - 0.5).round() as i64 + dr,
                        );
                        got[usize::try_from(r * 300 + c).unwrap()] == [1, EDGE_BIT | edge]
                    })
                });
                if !found {
                    missing.push((k, col, row));
                }
            }
            assert!(
                missing.is_empty(),
                "{projection:?}: Edge{edge} missing at {missing:?}"
            );
        }
    }
}

#[test]
fn an_up_nearly_along_an_oblique_view_keeps_the_roll_its_part_across_says() {
    // Was a break: the check takes an up whose part across the view is 1e-9 of its length or
    // more ("below it, the image's rotation about the view is lost in rounding"), but the
    // worker handed the up to look_at in f32, whose rounding of an oblique direction is
    // about 6e-8: an up 1e-8 across the view was rolled by rounding noise, not by its part
    // across.
    let mut w = worker();
    // The view's direction from the target to the eye is (1, −1, 1)/√3; the up's part
    // across it is the box's own up made perpendicular, (−1, 1, 2)/√6.
    let across = unit([-1.0, 1.0, 2.0]);
    let toward_eye = unit([1.0, -1.0, 1.0]);
    let want = ids(&mut w, &corner(100.0, across, ORTHO_50, 200));
    let up = [
        toward_eye[0] + 1e-8 * across[0],
        toward_eye[1] + 1e-8 * across[1],
        toward_eye[2] + 1e-8 * across[2],
    ];
    let scene = corner(100.0, up, ORTHO_50, 200);
    assert!(scene.check().is_ok());
    let got = ids(&mut w, &scene);
    assert_same_away_from_boundaries(&want, &got, 200, 200, "up 1e-8 across the view");
}

#[test]
fn an_orthographic_eye_a_kilometre_away_draws_as_one_near() {
    // An orthographic image doesn't depend on how far along its ray the eye is. 1e6 mm is
    // the document's largest length.
    let mut w = worker();
    let want = ids(&mut w, &corner(100.0, [0.0, 0.0, 1.0], ORTHO_50, 200));
    let got = ids(&mut w, &corner(1e6, [0.0, 0.0, 1.0], ORTHO_50, 200));
    assert_same_away_from_boundaries(&want, &got, 200, 200, "the eye 1e6 away");
}

#[test]
fn an_orthographic_eye_far_past_the_documents_lengths_draws_as_one_near_or_is_refused() {
    // Was a break: the eye's f32 translation, rotated in f32, moved the image by about
    // |eye| × 6e-8 (60 mm at 1e9, against 0.25 mm pixels), and the far plane at |eye| left
    // the depth buffer no precision: hidden edges were drawn. An orthographic view now uses
    // only the eye's direction, and an eye past MAX_RENDER_COORDINATE is refused.
    let mut w = worker();
    let want = ids(&mut w, &corner(100.0, [0.0, 0.0, 1.0], ORTHO_50, 200));
    for distance in [1e9, 1e15] {
        let scene = corner(distance, [0.0, 0.0, 1.0], ORTHO_50, 200);
        match call(&mut w, &scene) {
            Ok((_, got)) => assert_same_away_from_boundaries(
                &want,
                &got.unwrap(),
                200,
                200,
                &format!("the eye {distance:e} away"),
            ),
            Err(e) => assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}"),
        }
    }
}

#[test]
fn a_perspective_view_doesnt_depend_on_how_far_along_its_ray_the_target_is() {
    // Was a break: the near plane was 1e-4 of the eye's distance to the target, not of
    // what's in view, so a target 1e6 mm along the ray (the document's largest length) put
    // the near plane 100 mm out, past the box 80 mm away, and clipped it whole.
    let mut w = worker();
    let scene = |target_y: f64| RenderScene {
        bodies: vec![body(1, the_box())],
        camera: Camera {
            projection: Projection::Perspective {
                field_of_view: 40.0,
            },
            eye: [20.0, -80.0, 5.0],
            target: [20.0, target_y, 5.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 200,
        height: 200,
        ids: true,
    };
    let want = ids(&mut w, &scene(0.0));
    assert_eq!(want[100 * 200 + 100], FRONT_FACE);
    let got = ids(&mut w, &scene(1e6));
    assert_same_away_from_boundaries(&want, &got, 200, 200, "the target 1e6 along the ray");
}

// ---------------------------------------------------------------------------------------
// Random hostile scenes: refused exactly when the check refuses them, drawn otherwise with
// only the scene's own IDs, and the worker alive after every one.

/// Numbers at their ends: `wide`, every finite f64 (past f32's reach, about the target and
/// as a camera's numbers); otherwise only those the worker's f32 holds, to find what else
/// breaks.
fn hostile_f64(wide: bool) -> impl Strategy<Value = f64> {
    let ends = if wide {
        vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            1e-30,
            -1e-30,
            1e-300,
            5e-324,
            1e30,
            3.5e38,
            -3.5e38,
            1e300,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ]
    } else {
        vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            1e-3,
            -1e-3,
            1e6,
            -1e6,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ]
    };
    prop_oneof![
        6 => -60.0..60.0f64,
        1 => prop::sample::select(ends),
    ]
}

fn point(wide: bool) -> impl Strategy<Value = [f64; 3]> {
    prop_oneof![
        8 => [-30.0..30.0f64, -30.0..30.0f64, -30.0..30.0f64],
        1 => [hostile_f64(wide), hostile_f64(wide), hostile_f64(wide)],
    ]
}

fn normal() -> impl Strategy<Value = [f32; 3]> {
    prop_oneof![
        8 => [-1.0..1.0f32, -1.0..1.0f32, -1.0..1.0f32],
        1 => prop::sample::select(vec![
            [0.0f32; 3],
            [f32::NAN, 0.0, 0.0],
            [f32::INFINITY, 0.0, 1.0],
            [f32::MAX, f32::MAX, f32::MAX],
            [0.0, 0.0, -f32::MIN_POSITIVE],
        ]),
    ]
}

/// Counts that split `total` into `parts`, or (sometimes) don't add up to it.
fn counts(total: usize, parts: usize, cut: Vec<usize>, wrong: Option<u32>) -> Vec<u32> {
    let mut cut: Vec<usize> = cut
        .into_iter()
        .map(|c| c % (total + 1))
        .take(parts.saturating_sub(1))
        .collect();
    cut.sort_unstable();
    let mut out = Vec::new();
    let mut last = 0;
    for c in cut {
        out.push(u32::try_from(c - last).unwrap());
        last = c;
    }
    if parts > 0 {
        out.push(u32::try_from(total - last).unwrap());
    }
    if let (Some(extra), Some(first)) = (wrong, out.first_mut()) {
        *first = first.wrapping_add(extra);
    }
    out
}

fn mesh(wide: bool) -> impl Strategy<Value = Mesh> {
    (0usize..10)
        .prop_flat_map(move |n| {
            (
                prop::collection::vec(point(wide), n),
                prop::collection::vec(normal(), n),
                prop::collection::vec(
                    [
                        prop_oneof![9 => 0..(n as u32).max(1), 1 => any::<u32>()],
                        prop_oneof![9 => 0..(n as u32).max(1), 1 => any::<u32>()],
                        prop_oneof![9 => 0..(n as u32).max(1), 1 => any::<u32>()],
                    ],
                    0..8,
                ),
                0usize..5,
                prop::collection::vec(any::<usize>(), 4),
                prop::option::weighted(0.05, prop_oneof![Just(1u32), Just(u32::MAX)]),
                prop::collection::vec(point(wide), 0..10),
                0usize..5,
                prop::collection::vec(any::<usize>(), 4),
                prop::option::weighted(0.05, prop_oneof![Just(1u32), Just(u32::MAX)]),
                prop::bool::weighted(0.05),
            )
        })
        .prop_map(
            |(
                positions,
                mut normals,
                triangles,
                faces,
                fcut,
                fwrong,
                edge_points,
                edges,
                ecut,
                ewrong,
                odd,
            )| {
                if odd {
                    normals.pop();
                }
                let face_triangles = counts(triangles.len(), faces, fcut, fwrong);
                let edge_point_counts = counts(edge_points.len(), edges, ecut, ewrong);
                Mesh {
                    positions,
                    normals,
                    triangles,
                    face_triangles,
                    edge_points,
                    edge_point_counts,
                    valid: None,
                }
            },
        )
}

fn highlights() -> impl Strategy<Value = Vec<u32>> {
    prop_oneof![
        3 => Just(vec![]),
        3 => prop::collection::btree_set(0u32..6, 0..4).prop_map(|s| s.into_iter().collect()),
        1 => prop::collection::vec(prop_oneof![0u32..6, Just(u32::MAX)], 0..4),
    ]
}

fn camera(wide: bool) -> impl Strategy<Value = Camera> {
    let fovs = if wide {
        vec![180.0, 179.999_999, 1e-40]
    } else {
        vec![180.0, 179.9, 1e-3]
    };
    let projection = prop_oneof![
        3 => (1.0..200.0f64).prop_map(|view_height| Projection::Orthographic { view_height }),
        3 => (1.0..170.0f64).prop_map(|field_of_view| Projection::Perspective { field_of_view }),
        1 => hostile_f64(wide).prop_map(|view_height| Projection::Orthographic { view_height }),
        1 => prop_oneof![hostile_f64(wide), prop::sample::select(fovs)]
            .prop_map(|field_of_view| Projection::Perspective { field_of_view }),
    ];
    let up = prop_oneof![
        4 => Just([0.0, 0.0, 1.0]),
        2 => [-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64],
        1 => [hostile_f64(wide), hostile_f64(wide), hostile_f64(wide)],
    ];
    let eye = prop_oneof![
        4 => [-200.0..200.0f64, -200.0..-40.0f64, -200.0..200.0f64],
        1 => [hostile_f64(wide), hostile_f64(wide), hostile_f64(wide)],
    ];
    (projection, eye, point(wide), up).prop_map(|(projection, eye, target, up)| Camera {
        projection,
        eye,
        target,
        up,
    })
}

fn body_strategy(wide: bool) -> impl Strategy<Value = RenderBody> {
    (
        prop_oneof![9 => 1u32..5, 1 => prop::sample::select(vec![0, u32::MAX])],
        mesh(wide),
        highlights(),
        highlights(),
    )
        .prop_map(
            |(body, mesh, highlight_faces, highlight_edges)| RenderBody {
                body,
                mesh,
                highlight_faces,
                highlight_edges,
            },
        )
}

fn scene_strategy(wide: bool) -> impl Strategy<Value = RenderScene> {
    let side = prop_oneof![
        12 => 1u32..9,
        1 => prop::sample::select(vec![0, 4097, u32::MAX]),
    ];
    (
        prop::collection::vec(body_strategy(wide), 0..3),
        camera(wide),
        side.clone(),
        side,
        any::<bool>(),
    )
        .prop_map(|(mut bodies, camera, width, height, ids)| {
            // Mostly ascending numbers, so most scenes reach the meshes' checks.
            if bodies.len() > 1 && bodies[0].body < u32::MAX {
                bodies.sort_by_key(|b| b.body);
            }
            RenderScene {
                bodies,
                camera,
                width,
                height,
                ids,
            }
        })
}

/// Every ID a drawn scene may hold: the background, its bodies' faces with triangles, and
/// their edges with two points or more.
fn allowed(scene: &RenderScene) -> std::collections::BTreeSet<PixelId> {
    let mut out = std::collections::BTreeSet::from([[0, 0]]);
    for b in &scene.bodies {
        for (f, &count) in b.mesh.face_triangles.iter().enumerate() {
            if count > 0 {
                out.insert([b.body, u32::try_from(f + 1).unwrap()]);
            }
        }
        for (e, &count) in b.mesh.edge_point_counts.iter().enumerate() {
            if count >= 2 {
                out.insert([b.body, EDGE_BIT | u32::try_from(e + 1).unwrap()]);
            }
        }
    }
    out
}

#[test]
fn random_hostile_scenes_are_refused_exactly_as_the_check_says_or_drawn_with_their_own_ids() {
    // Was a break: finite numbers past f32's reach (a position or target of 1e39, a camera's of
    // 1e300) passed the check and failed in the worker as `Internal` ("Face1: Mesh has an
    // invalid bounding box Vec3(inf, ...)"): the targeted tests above, at random. Now
    // refused by the check, past MAX_RENDER_COORDINATE.
    hostile_scenes(true);
}

#[test]
fn random_hostile_scenes_in_f32s_reach_are_refused_exactly_as_the_check_says_or_drawn() {
    hostile_scenes(false);
}

fn hostile_scenes(wide: bool) {
    let w = RefCell::new(worker());
    // A fixed seed, so a red run is red every time.
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: if wide { 3000 } else { 1000 },
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let (drawn, refused) = (RefCell::new(0u32), RefCell::new(0u32));
    runner
        .run(&scene_strategy(wide), |scene| {
            let checked = scene.check();
            let result = call(&mut w.borrow_mut(), &scene);
            // The worker answers after every scene; one that doesn't is replaced, so a
            // failing case shrinks on a worker of its own.
            let echo = w.borrow_mut().call(
                Generation(1),
                Op::Echo {
                    data: silico_protocol::Blob::inline(vec![7]),
                },
                WAIT,
            );
            if echo.is_err() {
                *w.borrow_mut() = worker();
            }
            prop_assert!(echo.is_ok(), "the worker after it: {:?}", echo);
            match (checked, result) {
                (Ok(()), Ok((image, ids))) => {
                    *drawn.borrow_mut() += 1;
                    prop_assert_eq!((image.width, image.height), (scene.width, scene.height));
                    prop_assert_eq!(ids.is_some(), scene.ids);
                    if let Some(ids) = ids {
                        let allowed = allowed(&scene);
                        for id in &ids {
                            prop_assert!(allowed.contains(id), "{:?} isn't the scene's", id);
                        }
                    }
                }
                (Err(want), Err(e)) => {
                    *refused.borrow_mut() += 1;
                    prop_assert_eq!(e.code(), ErrorCode::InvalidInput, "{:?}", e);
                    prop_assert_eq!(e.parameter(), Some(want.parameter));
                }
                (Ok(()), Err(e)) => prop_assert!(false, "the check passed it; the worker: {:?}", e),
                (Err(want), Ok(_)) => prop_assert!(
                    false,
                    "the check refused it ({:?}); the worker drew it",
                    want
                ),
            }
            Ok(())
        })
        .unwrap();
    let (drawn, refused) = (*drawn.borrow(), *refused.borrow());
    // Both sides of the check were reached, many times.
    assert!(
        drawn > 200 && refused > 200,
        "drawn {drawn}, refused {refused}"
    );
}

/// Set in this test binary when it runs itself as a child with its environment cleared.
const NO_LAVAPIPE_CHILD: &str = "SILICO_RENDER_TEST_NO_LAVAPIPE";

#[test]
fn no_software_adapter_is_refused_as_data_not_as_a_bug_in_silico() {
    // Was a break: with no CPU adapter (lavapipe's compiled-in paths gone, or none built
    // in), the worker answered `Refusal::Internal`, which the core words as a bug in Silico
    // ("report it"). The design wants `worker_unavailable`, its cause saying lavapipe wasn't
    // found: now `Refusal::NoAdapter`, of kind `Unavailable`.
    //
    // The worker's own restart always sets lavapipe's variables, so the render worker's
    // state is driven here in this test binary, run again as a child with its environment
    // cleared and the Vulkan loader's drivers pointed at a file that isn't there (the
    // loader in pixi's environment otherwise finds lavapipe in its own prefix): it has no
    // CPU adapter.
    if std::env::var_os(NO_LAVAPIPE_CHILD).is_some() {
        let mut state =
            silico_render::worker::State::new(silico_render::worker::RenderAdapter::Software);
        match state.handle(Op::Render(Box::new(front_of_a_box(8, 8)))) {
            Ok(_) => panic!("NO_LAVAPIPE_CHILD drawn: a CPU adapter with no Vulkan driver"),
            Err(failure) => {
                let refusal = format!("{failure:?}");
                writeln!(std::io::stdout(), "NO_LAVAPIPE_CHILD refused: {refusal}").unwrap();
                assert!(
                    matches!(
                        &failure.refusal,
                        silico_protocol::Refusal::NoAdapter { cause }
                            if cause.contains("no software rasterizer was found")
                    ),
                    "a missing rasterizer refused as other than NoAdapter: {refusal}"
                );
            }
        }
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "adversarial_render::no_software_adapter_is_refused_as_data_not_as_a_bug_in_silico",
            "--nocapture",
        ])
        .env_clear()
        .env(NO_LAVAPIPE_CHILD, "1")
        .env("VK_ICD_FILENAMES", "/nonexistent/lvp_icd.json")
        .env("VK_DRIVER_FILES", "/nonexistent/lvp_icd.json")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("NO_LAVAPIPE_CHILD"),
        "the child didn't run: {text}"
    );
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_position_past_f32_about_the_target_is_refused_not_an_internal_error() {
    // 1e39 is finite, but about the target in f32 it's infinite (re_renderer would refuse the
    // face's mesh, an `Internal` error): refused as invalid input, naming the position, as
    // is anything past 1e12 mm from the origin; a camera target 1e39 away likewise.
    let mut w = worker();
    let mut mesh = the_box();
    mesh.positions[0] = [1e39, 0.0, 0.0];
    refused(
        &mut w,
        &with_mesh(32, 32, mesh),
        "bodies",
        "position 0 must be finite and within 1000000000000 mm",
    );
    let mut mesh = the_box();
    mesh.positions[0] = [1e12, 0.0, 0.0];
    let mut scene = with_mesh(32, 32, mesh);
    // A view tall enough for a point that far (1e-5 of its reach, at the least).
    scene.camera.projection = Projection::Orthographic { view_height: 1e8 };
    scene.check().unwrap();
    call(&mut w, &scene).unwrap();
    scene.camera.target = [1e39, 0.0, 0.0];
    refused(
        &mut w,
        &scene,
        "camera.target",
        "camera.target [1e39, 0.0, 0.0] must be finite and within",
    );
    still_draws(&mut w);
}

#[test]
fn three_thousand_bodies_draw_within_half_the_limit_each_with_its_number() {
    // A pattern of pins left unfused is a body each: each body is a line batch of its own
    // and each face a mesh of its own.
    let mut w = worker();
    // 18,000 faces, within MAX_RENDER_FACES.
    let n = 3000u32;
    let bodies: Vec<RenderBody> = (0..n)
        .map(|k| {
            let (i, j) = (f64::from(k % 100), f64::from(k / 100));
            body(k + 1, box_mesh([i * 0.4, 0.0, j * 0.2], [0.3, 0.3, 0.15]))
        })
        .collect();
    let scene = RenderScene {
        bodies,
        ..front_of_a_box(400, 300)
    };
    let (cost, result) = timed(&mut w, &scene);
    let (_, ids) = result.unwrap_or_else(|e| panic!("{cost}: {e:?}"));
    // Half the limit: a tenth (3 s) held unloaded, but loaded gate runners took 3.37 s,
    // 3.56 s, and 3.99 s of wall time (main at b896ede5, #164, #159), before the cost was
    // taken over the machine's slowdown. Half still fails a cost several times larger (one
    // size timed can't tell how it grows), and is what an agent's 30 s can count on.
    assert!(cost.cost < LIMIT / 2, "{cost}");
    // By hand: body k + 1's front face is x 0.4i to 0.4i + 0.3, z 0.2j to 0.2j + 0.15, about
    // the view's centre at (21, 7): the last row's first, j = 29 and i = 0 (body 2901), at
    // z 5.8 to 5.95; its front face's centre is (0.15, 5.875), 2 by 1 pixels, so its face's
    // or an edge's.
    let ids = ids.unwrap();
    let pixel = 300.0 / 44.0;
    let col = (200.0 + (0.15 - 21.0) * pixel) as usize;
    let row = (150.0 - (5.875 - 7.0) * pixel) as usize;
    assert_eq!(ids[row * 400 + col][0], 2901, "{:?}", ids[row * 400 + col]);
}

#[test]
fn the_same_scene_gives_the_same_id_buffer_in_one_worker_and_another() {
    let scene = corner(100.0, [0.0, 0.0, 1.0], ORTHO_50, 256);
    let mut w = worker();
    let first = ids(&mut w, &scene);
    // A larger render between, so the targets are remade.
    let _ = ids(&mut w, &front_of_a_box(1024, 768));
    let again = ids(&mut w, &scene);
    let other = ids(&mut worker(), &scene);
    assert!(first == again, "the same worker drew it differently");
    assert!(first == other, "another worker drew it differently");
}
