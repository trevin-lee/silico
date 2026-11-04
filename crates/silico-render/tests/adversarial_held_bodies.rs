//! Hostile scenes for the bodies the render worker holds between renders (`/break-it` on
//! crates/silico-render/src/render.rs: each body uploaded once about its own centre and
//! moved into each view by its instance's transform, the held bodies found again by number
//! and mesh, a face's plain and tinted meshes each uploaded the first time they're drawn).
//!
//! What's drawn is checked against a ray cast by hand in f64 here, or against a fresh
//! worker's drawing byte for byte, never against a stored image.

use std::cell::RefCell;
use std::path::Path;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Camera, Generation, Mesh, Op, PixelId, Projection, RenderBody, RenderScene, Reply,
};
use silico_render::bench::box_mesh;

use crate::common::WAIT;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-render-worker");

/// The front face of a box from `box_mesh` (y low), body 1.
const FRONT: PixelId = [1, 3];
const BACKGROUND: PixelId = [0, 0];

fn worker() -> Worker {
    Worker::spawn_as(Path::new(WORKER), WorkerKind::Render).unwrap()
}

/// `scene` drawn by `w`: the PNG's bytes as the worker sent them, and the ID buffer.
fn draw(w: &mut Worker, scene: &RenderScene) -> (Vec<u8>, Vec<PixelId>) {
    scene.check().unwrap();
    match w
        .call(Generation(1), Op::Render(Box::new(scene.clone())), WAIT)
        .unwrap_or_else(|e| panic!("refused: {e:?}"))
    {
        Reply::Rendered { png, ids } => {
            let ids = ids.unwrap();
            assert_eq!(
                ids.len() as u64,
                u64::from(scene.width) * u64::from(scene.height)
            );
            (png, ids)
        }
        other => panic!("Render replied {other:?}"),
    }
}

fn body(number: u32, mesh: Mesh) -> RenderBody {
    RenderBody {
        body: number,
        mesh,
        highlight_faces: vec![],
        highlight_edges: vec![],
    }
}

/// The ID at `(col, row)` of a `width`-wide buffer.
fn at(ids: &[PixelId], width: u32, col: u32, row: u32) -> PixelId {
    ids[usize::try_from(row * width + col).unwrap()]
}

// ---------------------------------------------------------------------------------------
// Precision: a body's positions are f32 about its own box's centre, so how precisely it's
// drawn depends on how big the body is, not on the view.

/// From the front (−y, z up), `distance` in front of the plane y = 0 at `(x, 0, 0.5)`,
/// perspective, `fov` degrees, 64 square.
fn front_close(x: f64, distance: f64, fov: f64, bodies: Vec<RenderBody>) -> RenderScene {
    RenderScene {
        bodies,
        camera: Camera {
            projection: Projection::Perspective { field_of_view: fov },
            eye: [x, -distance, 0.5],
            target: [x, 0.0, 0.5],
            up: [0.0, 0.0, 1.0],
        },
        width: 64,
        height: 64,
        ids: true,
    }
}

/// The columns, by hand, either side of a box's left end (x = 0) seen from the front at
/// `x_eye` and `distance`, `fov` degrees across 64 pixels: the last that must be the
/// background and the first that must be the front face, each 3 pixels clear of the end's
/// place, past its 2-pixel edge.
fn either_side_of_the_end(x_eye: f64, distance: f64, fov: f64) -> (u32, u32) {
    let pixel = 2.0 * distance * libm::tan((fov / 2.0).to_radians()) / 64.0;
    let end = 32.0 + (0.0 - x_eye) / pixel;
    ((end - 3.0) as u32, (end + 3.0).ceil() as u32)
}

#[test]
fn a_kilometre_rod_seen_close_at_its_end_draws_its_end_where_it_is() {
    // Was a break: the rod's end landed 14.7 pixels from where it is. The rod is 1e6 mm long
    // (the document's largest length) and 1 mm square, from the origin; the eye is 5 mm in
    // front of it, 0.01 mm right of its end, 0.5° across: a pixel is 2·5·tan(0.25°)/64 =
    // 6.8e-4 mm, so by hand the end (x = 0) is 0.01 mm, 14.7 pixels, left of the centre,
    // at column 17.3. Drawn about its box's centre (5e5, 0.5, 0.5), the end is −5e5 in f32
    // and the centre's offset from the eye, 499999.99, is 500000 in f32 (its step there is
    // 1/32): the end is drawn at the centre, Edge4 at columns 31 and 32, and columns 21 to
    // 30 are background. Drawn about the eye, as M4.6 drew it, the end is −0.01 in f32 to
    // 1e-9 mm, and M4.6's renderer (main at 39e9ddbb) passes this test and the two after it.
    // Now a body is drawn about the camera's point when a view is as close as this
    // (`View::anchor`).
    let rod = box_mesh([0.0; 3], [1e6, 1.0, 1.0]);
    let (x, distance, fov) = (0.01, 5.0, 0.5);
    let scene = front_close(x, distance, fov, vec![body(1, rod)]);
    let (_, ids) = draw(&mut worker(), &scene);
    let (outside, inside) = either_side_of_the_end(x, distance, fov);
    assert_eq!((outside, inside), (14, 21));
    assert_eq!(
        at(&ids, 64, outside, 32),
        BACKGROUND,
        "left of the rod's end"
    );
    let row: Vec<PixelId> = (0..64).map(|c| at(&ids, 64, c, 32)).collect();
    for col in inside..64 {
        assert_eq!(
            at(&ids, 64, col, 32),
            FRONT,
            "column {col}, right of the rod's end at 17.3: the row is {row:?}"
        );
    }
}

#[test]
fn a_kilometre_rod_at_the_least_orthographic_zoom_draws_its_end_where_it_is() {
    // Was a break: as in perspective, the end moved 4.1 pixels: orthographic, M4.6's least
    // view height for a reach of 1e6 (10 mm, 1e-5 of it), over 4096 rows: a pixel was
    // 10/4096 = 2.44e-3 mm, the end, 0.01 mm left of the target, drawn at the centre (the
    // centre's offset from the target, 499999.99, is 500000 in f32). That view is refused
    // now: at 4096 rows the least for the reach is 4096 × 999992.18 / 2^19 = 7812.44 mm (by
    // hand, the far corner (1e6, 1, −50) from (7.8217, 0, 0.5)). At 7814 mm (a pixel
    // 1.9077 mm, the rod 100 mm high so the row crosses it), the end, 7.8217 mm left of the
    // target, is at column 32 − 4.1 = 27.9 of 64 by hand.
    let at_height = |view_height: f64, x: f64, rod: Mesh| RenderScene {
        bodies: vec![body(1, rod)],
        camera: Camera {
            projection: Projection::Orthographic { view_height },
            eye: [x, -50.0, 0.5],
            target: [x, 0.0, 0.5],
            up: [0.0, 0.0, 1.0],
        },
        width: 64,
        height: 4096,
        ids: true,
    };
    let thin = box_mesh([0.0; 3], [1e6, 1.0, 1.0]);
    let refused = at_height(10.0, 0.01, thin);
    assert_eq!(refused.check().unwrap_err().parameter, "camera.view_height");
    let rod = box_mesh([0.0, 0.0, -50.0], [1e6, 1.0, 100.0]);
    let scene = at_height(7814.0, 4.1 * 7814.0 / 4096.0, rod);
    let (_, ids) = draw(&mut worker(), &scene);
    let row: Vec<PixelId> = (0..64).map(|c| at(&ids, 64, c, 2048)).collect();
    assert_eq!(at(&ids, 64, 25, 2048), BACKGROUND, "{row:?}");
    for col in 30..64 {
        assert_eq!(
            at(&ids, 64, col, 2048),
            FRONT,
            "column {col}, right of the rod's end at 27.9: the row is {row:?}"
        );
    }
}

#[test]
fn a_point_no_one_draws_a_kilometre_off_a_small_box_leaves_the_box_where_it_is() {
    // Was a break: one edge of a single point (drawn as nothing: an edge needs two) at x = 1e6
    // moves the box's centre to x = 5e5, and the 1 mm box, seen as in the rod's test, is
    // drawn 14.7 pixels from where it is. The point passes the check (within 1e12 mm), and
    // a perspective view has no zoom limit by reach.
    let (x, distance, fov) = (0.01, 5.0, 0.5);
    let (outside, inside) = either_side_of_the_end(x, distance, fov);
    let row_of = |ids: &[PixelId]| (0..64).map(|c| at(ids, 64, c, 32)).collect::<Vec<_>>();
    // The box alone: drawn where it is (its centre 0.5 from the eye's line, so its end is
    // about it to f32's 6e-8, a ten-thousandth of a pixel).
    let alone = box_mesh([0.0; 3], [1.0, 1.0, 1.0]);
    let scene = front_close(x, distance, fov, vec![body(1, alone.clone())]);
    let (_, ids) = draw(&mut worker(), &scene);
    assert_eq!(at(&ids, 64, outside, 32), BACKGROUND, "{:?}", row_of(&ids));
    for col in inside..64 {
        assert_eq!(
            at(&ids, 64, col, 32),
            FRONT,
            "column {col}, alone: {:?}",
            row_of(&ids)
        );
    }
    // With the point.
    let mut mesh = alone;
    mesh.edge_points.push([1e6, 0.5, 0.5]);
    mesh.edge_point_counts.push(1);
    let scene = front_close(x, distance, fov, vec![body(1, mesh)]);
    let (_, ids) = draw(&mut worker(), &scene);
    assert_eq!(at(&ids, 64, outside, 32), BACKGROUND, "{:?}", row_of(&ids));
    for col in inside..64 {
        assert_eq!(
            at(&ids, 64, col, 32),
            FRONT,
            "column {col}, with a point 1e6 off: {:?}",
            row_of(&ids)
        );
    }
}

// ---------------------------------------------------------------------------------------
// The held bodies: found again by number and mesh, each scene drawn as a fresh worker
// draws it, the PNG byte for byte.

/// A small box at `x`.
fn small_at(x: f64) -> Mesh {
    box_mesh([x, 0.0, 0.0], [10.0, 10.0, 10.0])
}

/// From the front, orthographic, 60 mm high about (50, 0, 5), 160 by 96: x 0 to 100.
fn front_of(bodies: Vec<RenderBody>) -> RenderScene {
    RenderScene {
        bodies,
        camera: Camera {
            projection: Projection::Orthographic { view_height: 60.0 },
            eye: [50.0, -100.0, 5.0],
            target: [50.0, 0.0, 5.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 160,
        height: 96,
        ids: true,
    }
}

/// The column of x in [`front_of`], at its row 48.
fn column_of(x: f64) -> u32 {
    let pixel = 60.0 / 96.0;
    (80.0 + (x - 50.0) / pixel) as u32
}

#[test]
fn two_bodies_that_swap_meshes_are_each_drawn_by_its_own_number() {
    // Body 1 at x 0 and body 2 at x 90, then the other way about: each held mesh is found
    // under the other's number, and must not be drawn under its old one.
    let mut held = worker();
    let first = front_of(vec![body(1, small_at(0.0)), body(2, small_at(90.0))]);
    let (_, ids) = draw(&mut held, &first);
    assert_eq!(at(&ids, 160, column_of(5.0), 48), FRONT);
    assert_eq!(at(&ids, 160, column_of(95.0), 48), [2, 3]);
    let swapped = front_of(vec![body(1, small_at(90.0)), body(2, small_at(0.0))]);
    let (png, ids) = draw(&mut held, &swapped);
    assert_eq!(at(&ids, 160, column_of(5.0), 48), [2, 3]);
    assert_eq!(at(&ids, 160, column_of(95.0), 48), FRONT);
    let (fresh, _) = draw(&mut worker(), &swapped);
    assert!(
        png == fresh,
        "the swapped scene's PNG differs from a fresh worker's"
    );
}

#[test]
fn every_scene_in_turn_draws_byte_for_byte_as_a_fresh_worker_draws_it() {
    // Each scene's PNG and ID buffer from one worker that holds the bodies before it,
    // against a fresh worker's: views changed, highlights on, off, and on others, a mesh
    // changed only in its normals (its light), only in −0 for 0 (the same mesh to `==`,
    // drawn the same), bodies dropped and brought back, and a scene of none.
    let the_box = || box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
    let corner = |eye: [f64; 3], projection: Projection, bodies: Vec<RenderBody>| RenderScene {
        bodies,
        camera: Camera {
            projection,
            eye,
            target: [20.0, 10.0, 5.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 96,
        height: 80,
        ids: true,
    };
    let ortho = Projection::Orthographic { view_height: 60.0 };
    let wide = Projection::Perspective {
        field_of_view: 50.0,
    };
    let lit = |faces: Vec<u32>, edges: Vec<u32>| RenderBody {
        highlight_faces: faces,
        highlight_edges: edges,
        ..body(1, the_box())
    };
    let mut inward = the_box();
    for n in &mut inward.normals[8..12] {
        *n = [0.0, 1.0, 0.0];
    }
    let mut negative_zero = the_box();
    for p in negative_zero
        .positions
        .iter_mut()
        .chain(&mut negative_zero.edge_points)
    {
        for c in p.iter_mut() {
            if *c == 0.0 {
                *c = -0.0;
            }
        }
    }
    assert!(negative_zero == the_box(), "−0 isn't 0 to the mesh's `==`");
    let a = [80.0, -60.0, 50.0];
    let b = [-40.0, -70.0, 30.0];
    let c = [25.0, 5.0, 90.0];
    let scenes = [
        corner(a, ortho, vec![body(1, the_box())]),
        corner(b, wide, vec![body(1, the_box())]),
        corner(b, wide, vec![lit(vec![2, 5], vec![0, 3])]),
        corner(a, ortho, vec![lit(vec![2, 5], vec![0, 3])]),
        corner(a, ortho, vec![lit(vec![0, 1, 2, 3, 4, 5], vec![])]),
        corner(c, wide, vec![body(1, the_box())]),
        corner(a, ortho, vec![body(1, inward.clone())]),
        corner(b, ortho, vec![body(1, inward)]),
        corner(a, ortho, vec![body(1, negative_zero)]),
        corner(a, ortho, vec![]),
        corner(b, wide, vec![lit(vec![2], vec![]), body(7, small_at(30.0))]),
        corner(c, ortho, vec![body(7, small_at(30.0))]),
        corner(a, ortho, vec![body(1, the_box())]),
    ];
    let mut held = worker();
    for (i, s) in scenes.iter().enumerate() {
        let (png, ids) = draw(&mut held, s);
        let (fresh_png, fresh_ids) = draw(&mut worker(), s);
        assert!(
            ids == fresh_ids,
            "scene {i}: the ID buffer differs from a fresh worker's"
        );
        assert!(
            png == fresh_png,
            "scene {i}: the PNG differs from a fresh worker's"
        );
    }
}

#[test]
fn a_mesh_changed_only_in_its_normals_is_lit_by_its_new_ones() {
    // Held by number, a body whose normals alone changed must be uploaded again: its front
    // face turned inward is lit at 0.2 + 0.5 × 5/√35 of its colour, not at the outward
    // normal's full light (the light's law in the design, section *What a render looks
    // like*): darker in every channel.
    let mut held = worker();
    let scene = |mesh: Mesh| RenderScene {
        bodies: vec![body(1, mesh)],
        ..front_of(vec![])
    };
    let decode = |png: &[u8]| {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut rgb = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut rgb).unwrap();
        rgb
    };
    let pixel = |rgb: &[u8], col: u32, row: u32| {
        let i = usize::try_from((row * 160 + col) * 3).unwrap();
        [rgb[i], rgb[i + 1], rgb[i + 2]]
    };
    let outward = small_at(0.0);
    let (png, ids) = draw(&mut held, &scene(outward.clone()));
    let col = column_of(5.0);
    assert_eq!(at(&ids, 160, col, 48), FRONT);
    let bright = pixel(&decode(&png), col, 48);
    let mut inward = outward;
    for n in &mut inward.normals[8..12] {
        *n = [0.0, 1.0, 0.0];
    }
    let (png, _) = draw(&mut held, &scene(inward.clone()));
    let dark = pixel(&decode(&png), col, 48);
    for k in 0..3 {
        assert!(dark[k] < bright[k], "{dark:?} not darker than {bright:?}");
    }
    let (fresh, _) = draw(&mut worker(), &scene(inward));
    assert!(png == fresh, "differs from a fresh worker's");
}

// ---------------------------------------------------------------------------------------
// Random views of a box anywhere the check allows, two views each from one worker (the
// second from the meshes the first uploaded), every pixel clear of a boundary checked
// against a ray cast by hand.

/// What a ray from `origin` along `dir` hits first of the box from 0 to `size`: its face's
/// number (x low 1, x high 2, y low 3, y high 4, z low 5, z high 6), or none.
fn hit(origin: [f64; 3], dir: [f64; 3], size: [f64; 3]) -> Option<u32> {
    let (mut near, mut far, mut face) = (f64::NEG_INFINITY, f64::INFINITY, None);
    for k in 0..3 {
        if dir[k] == 0.0 {
            if origin[k] < 0.0 || origin[k] > size[k] {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((0.0 - origin[k]) / dir[k], (size[k] - origin[k]) / dir[k]);
        let (enter, exit, side) = if t0 < t1 { (t0, t1, 0) } else { (t1, t0, 1) };
        if enter > near {
            near = enter;
            face = Some(2 * u32::try_from(k).unwrap() + 1 + side);
        }
        far = far.min(exit);
    }
    (near <= far && near > 0.0).then_some(face).flatten()
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
    a.map(|c| c / l)
}

/// The ID the pixel at `(u, v)` (in pixels from the top left, centres at half-integers)
/// sees of the box at `low` of `size`, by a ray cast in f64 about the box's low corner.
fn cast(scene: &RenderScene, low: [f64; 3], size: [f64; 3], u: f64, v: f64) -> PixelId {
    let c = &scene.camera;
    let forward = unit(sub(c.target, c.eye));
    let right = unit(cross(forward, c.up));
    let up = cross(right, forward);
    let (w, h) = (f64::from(scene.width), f64::from(scene.height));
    let (x, y) = (2.0 * u / w - 1.0, 1.0 - 2.0 * v / h);
    let along = |o: [f64; 3], a: [f64; 3], s: f64| std::array::from_fn(|k| o[k] + a[k] * s);
    let (origin, dir) = match c.projection {
        Projection::Orthographic { view_height } => {
            let half = view_height / 2.0;
            let on = along(along(c.target, right, x * half * w / h), up, y * half);
            (along(on, forward, -1e7), forward)
        }
        Projection::Perspective { field_of_view } => {
            let t = libm::tan((field_of_view / 2.0).to_radians());
            let d = along(along(forward, right, x * t * w / h), up, y * t);
            (c.eye, d)
        }
    };
    match hit(sub(origin, low), dir, size) {
        Some(face) => [1, face],
        None => [0, 0],
    }
}

/// How far about a pixel's centre [`assert_cast`] casts, in half pixels: 2.5 pixels out,
/// past an edge's 2, every half pixel, finer than the thinnest box [`scene_of`] makes.
const AROUND: i64 = 5;

/// Every pixel of `ids` whose ray and those every half pixel up to 2.5 pixels about it all
/// see the same thing (so no face's boundary nor an edge's 2 pixels is near it) must be
/// what they see. How many of those see the box.
fn assert_cast(
    scene: &RenderScene,
    low: [f64; 3],
    size: [f64; 3],
    ids: &[PixelId],
    what: &str,
) -> usize {
    // The casts on a grid of half pixels, from 2.5 pixels outside the image's top left.
    let (w, h) = (i64::from(scene.width), i64::from(scene.height));
    let (gw, gh) = (2 * w + 2 * AROUND + 1, 2 * h + 2 * AROUND + 1);
    let grid: Vec<PixelId> = (0..gh)
        .flat_map(|j| (0..gw).map(move |i| (i, j)))
        .map(|(i, j)| {
            let half = |n: i64| 0.5 * (n - AROUND) as f64;
            cast(scene, low, size, half(i), half(j))
        })
        .collect();
    let seen = |i: i64, j: i64| grid[usize::try_from(j * gw + i).unwrap()];
    let (mut checked, mut on_the_box, mut wrong) = (0usize, 0usize, Vec::new());
    for row in 0..h {
        for col in 0..w {
            // The pixel's centre, (col + 0.5, row + 0.5), on the grid.
            let (i, j) = (2 * col + 1 + AROUND, 2 * row + 1 + AROUND);
            let want = seen(i, j);
            let clear = (-AROUND..=AROUND)
                .all(|di| (-AROUND..=AROUND).all(|dj| seen(i + di, j + dj) == want));
            if !clear {
                continue;
            }
            checked += 1;
            on_the_box += usize::from(want != BACKGROUND);
            let got = ids[usize::try_from(row * w + col).unwrap()];
            if got != want {
                wrong.push(((col, row), got, want));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{what}: {} of {checked} pixels clear of a boundary differ (at, got, want): {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(5)]
    );
    on_the_box
}

/// A finite f64 biased to its ends: 0, ±tiny, and the large offsets where f32 runs out.
fn offset() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(1e9),
        Just(-1e9),
        Just(3e11),
        Just(-7e11),
        Just(9.99e11),
        -1e6..1e6f64,
        -1e11..1e11f64,
    ]
}

/// A direction from the box to the eye, random but at least 0.1 off the up it's given,
/// as the views an orbit passes through (along the axes too).
fn direction() -> impl Strategy<Value = [f64; 3]> {
    prop_oneof![
        Just([0.0, -1.0, 0.0]),
        Just([1.0, 0.0, 0.0]),
        Just([0.0, 0.0, 1.0]),
        Just([0.0, 0.0, -1.0]),
        Just([-1.0, 1.0, -1.0]),
        prop::array::uniform3(-1.0..1.0f64),
    ]
}

/// One view of a box about its centre: a direction, a distance as a multiple of the box's
/// half diagonal, a roll, and a projection's angle or height.
#[derive(Debug, Clone)]
struct View {
    toward: [f64; 3],
    up: [f64; 3],
    away: f64,
    perspective: bool,
    fov: f64,
}

fn view() -> impl Strategy<Value = View> {
    (
        direction(),
        prop::array::uniform3(-1.0..1.0f64),
        1.6..6.0f64,
        any::<bool>(),
        10.0..100.0f64,
    )
        .prop_map(|(toward, up, away, perspective, fov)| View {
            toward,
            up,
            away,
            perspective,
            fov,
        })
}

/// The scene of the box at `low` of `size` from `view`, or none if the view's numbers
/// make no camera (a direction of no length, or an up along it).
fn scene_of(low: [f64; 3], size: [f64; 3], v: &View) -> Option<RenderScene> {
    if dot(v.toward, v.toward) < 1e-4 {
        return None;
    }
    let toward = unit(v.toward);
    let across = cross(toward, v.up);
    if dot(across, across).sqrt() < 0.1 * dot(v.up, v.up).sqrt() || dot(v.up, v.up) < 1e-4 {
        return None;
    }
    let target: [f64; 3] = std::array::from_fn(|k| low[k] + size[k] / 2.0);
    let r = dot(size, size).sqrt() / 2.0;
    let eye = std::array::from_fn(|k| target[k] + toward[k] * r * v.away);
    let projection = if v.perspective {
        Projection::Perspective {
            field_of_view: v.fov,
        }
    } else {
        Projection::Orthographic {
            view_height: r * v.fov / 20.0,
        }
    };
    let scene = RenderScene {
        bodies: vec![body(1, box_mesh(low, size))],
        camera: Camera {
            projection,
            eye,
            target,
            up: v.up,
        },
        width: 48,
        height: 40,
        ids: true,
    };
    scene.check().is_ok().then_some(scene)
}

#[test]
fn random_views_of_a_box_anywhere_draw_every_clear_pixel_as_a_ray_cast_by_hand_sees_it() {
    let w = RefCell::new(worker());
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 1000,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let strategy = (
        prop::array::uniform3(offset()),
        prop_oneof![Just(1.0), 0.5..20.0f64],
        prop::array::uniform3(prop_oneof![Just(1.0), 1.0..3.0f64]),
        view(),
        view(),
    );
    let drawn = std::cell::Cell::new(0usize);
    // The views in which some pixel clear of every boundary is the box's.
    let seen = std::cell::Cell::new(0usize);
    runner
        .run(&strategy, |(low, base, shape, first, second)| {
            // Each side 1 to 3 times the base: the thinnest is at least 2/√19 = 0.46 of the
            // box's half diagonal, 3.7 pixels in the widest view (5 half diagonals over 40
            // rows), so no part of it falls between the casts.
            let size = shape.map(|s| s * base);
            for (k, v) in [first, second].iter().enumerate() {
                let Some(scene) = scene_of(low, size, v) else {
                    continue;
                };
                let (_, ids) = draw(&mut w.borrow_mut(), &scene);
                drawn.set(drawn.get() + 1);
                let on = assert_cast(
                    &scene,
                    low,
                    size,
                    &ids,
                    &format!(
                        "view {k} of the box at {low:?} of {size:?}: {:?}",
                        scene.camera
                    ),
                );
                if on > 0 {
                    seen.set(seen.get() + 1);
                }
            }
            Ok(())
        })
        .unwrap();
    assert!(drawn.get() > 1500, "only {} views drawn", drawn.get());
    // A view drawing nothing would pass every pixel it checks as background: two thirds
    // of the views must check some pixel of the box (1543 of 1996 do; the rest see it
    // small, a wide angle from afar, or edge-on, too thin to be clear of its boundaries).
    assert!(
        seen.get() * 3 > drawn.get() * 2,
        "the box seen clear of its boundaries in only {} of {} views",
        seen.get(),
        drawn.get()
    );
}
