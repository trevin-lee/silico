//! Hostile scenes for the point each face is drawn about (`/break-it` on
//! crates/silico-render/src/render.rs at 2ec921e6: each face uploaded about its own box's
//! centre, `View::anchor` choosing per face and per view between that centre and the
//! camera's point by the corner's sec²; `settle` dropping a face's meshes about another
//! point or in the colour it isn't drawn in; and the edges built each view about the
//! camera's point). Its cases at the check's own limits (a field of view of 0.001°, the
//! least orthographic zoom on a kilometre slab, a near plane a millionth of a kilometre's
//! reach), which fail as M4.6's renderer failed them, wait on the maintainer, on the
//! branch `break-it-render-check-limits`.
//!
//! What's drawn is checked against a projection or a ray cast by hand in f64 here, or
//! against a fresh worker's drawing byte for byte, never against a stored image.

use std::path::Path;

use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Camera, EDGE_BIT, Generation, MIN_VIEW_HEIGHT_OF_REACH, Mesh, Op, PixelId, Projection,
    RenderBody, RenderScene, Reply,
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

/// `scene` drawn by `w`: the PNG's bytes, and the ID buffer.
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

fn at(ids: &[PixelId], width: u32, col: u32, row: u32) -> PixelId {
    ids[usize::try_from(row * width + col).unwrap()]
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
fn along(o: [f64; 3], a: [f64; 3], s: f64) -> [f64; 3] {
    std::array::from_fn(|k| o[k] + a[k] * s)
}

/// The camera's basis by hand: forward from the eye to the target, right = forward × up,
/// up = right × forward.
fn basis(c: &Camera) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let forward = unit(sub(c.target, c.eye));
    let right = unit(cross(forward, c.up));
    (forward, right, cross(right, forward))
}

/// Where `p` lands in `scene`'s image, in pixels (column, row), centres at half-integers.
fn pixel(scene: &RenderScene, p: [f64; 3]) -> (f64, f64) {
    let c = &scene.camera;
    let (forward, right, up) = basis(c);
    let (w, h) = (f64::from(scene.width), f64::from(scene.height));
    let (x, y, scale) = match c.projection {
        Projection::Orthographic { view_height } => {
            let d = sub(p, c.target);
            (dot(d, right), dot(d, up), h / view_height)
        }
        Projection::Perspective { field_of_view } => {
            let d = sub(p, c.eye);
            let depth = dot(d, forward);
            let tan = libm::tan(field_of_view.to_radians() / 2.0);
            (dot(d, right) / depth, dot(d, up) / depth, h / 2.0 / tan)
        }
    };
    (w / 2.0 + x * scale, h / 2.0 - y * scale)
}

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

/// The ID the pixel at `(u, v)` (in pixels from the top left) sees of the box at `low` of
/// `size`, by a ray cast in f64 about the box's low corner.
fn cast(scene: &RenderScene, low: [f64; 3], size: [f64; 3], u: f64, v: f64) -> PixelId {
    let c = &scene.camera;
    let (forward, right, up) = basis(c);
    let (w, h) = (f64::from(scene.width), f64::from(scene.height));
    let (x, y) = (2.0 * u / w - 1.0, 1.0 - 2.0 * v / h);
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
        None => BACKGROUND,
    }
}

/// Whether the pixel at `(col, row)` is within 2.5 pixels of where the edge `edge` of
/// `scene`'s first body projects: an edge is drawn 2 pixels wide however thin its faces
/// are, so beside a body thinner than the casts' half pixel its pixels are the edge's.
fn near_edge(scene: &RenderScene, edge: u32, col: i64, row: i64) -> bool {
    let mesh = &scene.bodies[0].mesh;
    let Ok(i) = usize::try_from(2 * (edge.wrapping_sub(1))) else {
        return false;
    };
    let (Some(&a), Some(&b)) = (mesh.edge_points.get(i), mesh.edge_points.get(i + 1)) else {
        return false;
    };
    let (forward, _, _) = basis(&scene.camera);
    let (x, y) = (col as f64 + 0.5, row as f64 + 0.5);
    let steps = (0..=1000)
        .map(|k| f64::from(k) / 1000.0)
        .chain((0..300).flat_map(|k| {
            let t = libm::pow(10.0, -f64::from(k) / 20.0);
            [t, 1.0 - t]
        }));
    steps.into_iter().any(|t| {
        let q = std::array::from_fn(|n| a[n] + (b[n] - a[n]) * t);
        if matches!(scene.camera.projection, Projection::Perspective { .. })
            && dot(sub(q, scene.camera.eye), forward) <= 0.0
        {
            return false;
        }
        let (c, r) = pixel(scene, q);
        (c - x).abs() <= 2.5 && (r - y).abs() <= 2.5
    })
}

/// A pixel that differs from what it should be: where (column, row), what's drawn, and
/// what should be.
type Miss = ((i64, i64), PixelId, PixelId);

/// How far about a pixel's centre [`cast_misses`] casts, in half pixels: 2.5 pixels out,
/// past an edge's 2.
const AROUND: i64 = 5;

/// Every `stride`th pixel each way whose ray and those every half pixel up to 2.5 pixels
/// about it all see the same thing must be what they see; the pixels that differ (at, got,
/// want), and how many were checked on the box.
fn cast_misses(
    scene: &RenderScene,
    low: [f64; 3],
    size: [f64; 3],
    ids: &[PixelId],
    stride: usize,
) -> (Vec<Miss>, usize) {
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
    let (mut on_the_box, mut wrong) = (0usize, Vec::new());
    for row in (0..h).step_by(stride) {
        for col in (0..w).step_by(stride) {
            let (i, j) = (2 * col + 1 + AROUND, 2 * row + 1 + AROUND);
            let want = seen(i, j);
            let clear = (-AROUND..=AROUND)
                .all(|di| (-AROUND..=AROUND).all(|dj| seen(i + di, j + dj) == want));
            if !clear {
                continue;
            }
            on_the_box += usize::from(want != BACKGROUND);
            let got = ids[usize::try_from(row * w + col).unwrap()];
            if got != want {
                wrong.push(((col, row), got, want));
            }
        }
    }
    (wrong, on_the_box)
}

// ---------------------------------------------------------------------------------------
// Orthographic: the anchor's choice by the target's distance from the face.

/// The kilometre rod (1e6 × 1 × 1 from the origin) from the front, orthographic, 11.67 mm
/// high over 4096 rows, 64 wide, the view's centre at x = 0.01, z = 0.5, its target `y`
/// along the view's ray (the image doesn't depend on it: only the eye's direction counts).
fn rod_from_the_front(y: f64) -> RenderScene {
    let target = [0.01, y, 0.5];
    RenderScene {
        bodies: vec![body(1, box_mesh([0.0; 3], [1e6, 1.0, 1.0]))],
        camera: Camera {
            projection: Projection::Orthographic { view_height: 11.67 },
            eye: [target[0], target[1] - 10.0, target[2]],
            target,
            up: [0.0, 0.0, 1.0],
        },
        width: 64,
        height: 4096,
        ids: true,
    }
}

#[test]
fn an_orthographic_target_far_along_the_ray_from_a_long_face_draws_its_end_where_it_is() {
    // Was a break: a target 6e5 in front of (or behind) the rod's front face is farther from the
    // face's box than its half diagonal (5e5), so `View::anchor` keeps the face about its
    // centre (5e5, 0, 0.5) though f32 about it misses (5e5 × 2^-22 = 0.12 mm against an
    // eighth of a 2.85e-3 mm pixel). The centre's offset from the target across the view,
    // 499999.99, is 500000 in f32 (its step there is 1/32): the face's end is drawn at the
    // view's centre, column 32, not 3.5 pixels left of it, where the end's edge (about the
    // camera's point, exact) is. Columns 30 and 31 are drawn background. With the target
    // on the face (y = 0, nearer than the half diagonal) the face is drawn about the target
    // and is right; M4.6's renderer (about the target always) drew all three the same.
    //
    // By hand: the view's reach from (0.01, ±6e5, 0.5) is |(1e6, 6e5 + 1, 0.5)| = 1.166e6,
    // so its least height is 11.66 mm, and 11.67 passes; a pixel is 11.67 / 4096 =
    // 2.849e-3 mm, the end (x = 0) 0.01 mm left of the centre, at column 32 − 3.51 = 28.49;
    // the edge there is 2 pixels wide, so from column 30 on, row 2048 (z = 0.5) is the
    // front face.
    let mut w = worker();
    for y in [0.0, -6e5, 6e5] {
        let scene = rod_from_the_front(y);
        let (col, _) = pixel(&scene, [0.0, 0.0, 0.5]);
        assert!((col - 28.49).abs() < 0.01, "{col}");
        let (_, ids) = draw(&mut w, &scene);
        let row: Vec<PixelId> = (0..64).map(|c| at(&ids, 64, c, 2048)).collect();
        assert_eq!(row[25], BACKGROUND, "the target at y = {y}: {row:?}");
        for c in 30..64 {
            assert_eq!(
                row[c], FRONT,
                "the target at y = {y}, column {c}, right of the end at 28.49: {row:?}"
            );
        }
    }
}

#[test]
fn an_orthographic_image_doesnt_depend_on_where_along_its_ray_the_target_is() {
    // Was a break (as above, the same cause): every pixel clear of a boundary is the same with
    // the target on the rod's face and 6e5 along the ray either way, as the design says of
    // an orthographic view (only the eye's direction counts).
    let mut w = worker();
    let (_, want) = draw(&mut w, &rod_from_the_front(0.0));
    for y in [-6e5, 6e5] {
        let (_, got) = draw(&mut w, &rod_from_the_front(y));
        let differ: Vec<(usize, PixelId, PixelId)> = (0..want.len())
            .filter(|&i| {
                let (c, r) = ((i % 64) as i64, (i / 64) as i64);
                got[i] != want[i]
                    && (-2..=2).all(|dc: i64| {
                        (-2..=2).all(|dr: i64| {
                            let (cc, rr) = (c + dc, r + dr);
                            !(0..64).contains(&cc)
                                || !(0..4096).contains(&rr)
                                || want[usize::try_from(rr * 64 + cc).unwrap()] == want[i]
                        })
                    })
            })
            .map(|i| (i, want[i], got[i]))
            .collect();
        assert!(
            differ.is_empty(),
            "the target at y = {y}: {} pixels clear of a boundary differ, the first {:?}",
            differ.len(),
            &differ[..differ.len().min(5)]
        );
    }
}

// ---------------------------------------------------------------------------------------
// The edges, about the camera's point, against the faces, about their centres.

/// A box `size` on a side from the origin seen from 99,999 mm off along (1, 2, −2) (the eye
/// at its centre + 33333 × (1, 2, −2)), up z, perspective at `field_of_view`, 384 by 4096:
/// at 0.001°, the narrowest the check allows, a pixel there is 99999 × 2 tan(0.0005°) /
/// 4096 = 4.26e-4 mm, and a 0.1 mm box about 235 pixels across.
fn far_narrow_view(field_of_view: f64, size: f64) -> RenderScene {
    let centre = [size / 2.0; 3];
    RenderScene {
        bodies: vec![body(1, box_mesh([0.0; 3], [size; 3]))],
        camera: Camera {
            projection: Projection::Perspective { field_of_view },
            eye: [
                centre[0] + 33333.0,
                centre[1] + 66666.0,
                centre[2] - 66666.0,
            ],
            target: centre,
            up: [0.0, 0.0, 1.0],
        },
        width: 384,
        height: 4096,
        ids: true,
    }
}

/// The edges of a box from `box_mesh` seen from (+x, +y, −z) of it: all but those of its
/// faces 1, 3, and 6 alone (x low, y low, z high), Edge3, Edge4, and Edge12.
const SEEN_FROM_BELOW: [u32; 9] = [1, 2, 5, 6, 7, 8, 9, 10, 11];

/// The tenths along each edge in `edges` whose projection by hand has no pixel of the edge's
/// ID within 1.5 pixels: (edge, tenth, column, row).
fn edges_off_their_projection(
    scene: &RenderScene,
    ids: &[PixelId],
    edges: &[u32],
) -> Vec<(u32, u32, f64, f64)> {
    let mesh = &scene.bodies[0].mesh;
    let (w, h) = (i64::from(scene.width), i64::from(scene.height));
    let mut off = vec![];
    for &edge in edges {
        let id = [1, EDGE_BIT | edge];
        let i = usize::try_from(2 * (edge - 1)).unwrap();
        let (a, b) = (mesh.edge_points[i], mesh.edge_points[i + 1]);
        for k in 2..9u32 {
            let t = f64::from(k) / 10.0;
            let q = std::array::from_fn(|n| a[n] + (b[n] - a[n]) * t);
            let (col, row) = pixel(scene, q);
            let found = (-1..=1).any(|dr: i64| {
                (-1..=1).any(|dc: i64| {
                    let (c, r) = (
                        (col - 0.5).round() as i64 + dc,
                        (row - 0.5).round() as i64 + dr,
                    );
                    (0..w).contains(&c)
                        && (0..h).contains(&r)
                        && ids[usize::try_from(r * w + c).unwrap()] == id
                })
            });
            if !found {
                off.push((edge, k, col, row));
            }
        }
    }
    off
}

#[test]
fn at_a_usual_field_of_view_the_same_box_draws_its_faces_and_edges_where_they_are() {
    // The helpers' own check: the same box from the same direction 20 mm off at 1°, 256 by
    // 256 (a pixel 20 × 2 tan 0.5° / 256 = 1.36e-3 mm, the box about 70 pixels across):
    // f32 about the eye moves 20 mm by about 1e-6 mm, a thousandth of a pixel.
    let mut w = worker();
    let mut scene = far_narrow_view(1.0, 0.1);
    let centre = [0.05; 3];
    let d = unit([1.0, 2.0, -2.0]);
    scene.camera.eye = along(centre, d, 20.0);
    scene.width = 256;
    scene.height = 256;
    let (_, ids) = draw(&mut w, &scene);
    let (wrong, on) = cast_misses(&scene, [0.0; 3], [0.1; 3], &ids, 1);
    assert!(on > 1_500, "{on}");
    assert!(wrong.is_empty(), "{:?}", &wrong[..wrong.len().min(5)]);
    let off = edges_off_their_projection(&scene, &ids, &SEEN_FROM_BELOW);
    assert!(off.is_empty(), "{off:?}");
    for hidden in [3, 4, 12] {
        assert!(!ids.contains(&[1, EDGE_BIT | hidden]), "Edge{hidden} drawn");
    }
}

// ---------------------------------------------------------------------------------------
// The held meshes, as faces cross between their centres and the camera's point.

#[test]
fn a_sequence_that_moves_faces_between_their_centres_and_the_eye_draws_as_a_fresh_worker_does() {
    // The rod seen at its end (its long faces about the eye, re-uploaded each view; its
    // ends about their centres), then from farther (all about their centres), then close
    // again, with highlights turned on and off as faces move between the two points: each
    // scene's PNG and ID buffer from one worker holding the meshes before it against a
    // fresh worker's, byte for byte.
    let rod = box_mesh([0.0; 3], [1e6, 20.0, 10.0]);
    let view = |x: f64, distance: f64, faces: Vec<u32>, edges: Vec<u32>| RenderScene {
        bodies: vec![RenderBody {
            highlight_faces: faces,
            highlight_edges: edges,
            ..body(1, rod.clone())
        }],
        camera: Camera {
            projection: Projection::Perspective { field_of_view: 0.5 },
            eye: [x, -distance, 5.0],
            target: [x, 0.0, 5.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 64,
        height: 64,
        ids: true,
    };
    let scenes = [
        view(0.01, 5.0, vec![], vec![]),
        view(0.01, 5.0, vec![2], vec![3]),
        view(0.012, 5.0, vec![2, 3], vec![]),
        view(0.012, 5e5, vec![2], vec![]),
        view(0.012, 5e5, vec![], vec![0, 3]),
        view(0.011, 5.0, vec![], vec![0, 3]),
        view(0.011, 5.0, vec![0, 1, 2, 3, 4, 5], vec![]),
        view(0.01, 5.0, vec![], vec![]),
    ];
    let mut held = worker();
    for (i, s) in scenes.iter().enumerate() {
        let (png, ids) = draw(&mut held, s);
        let (fresh_png, fresh_ids) = draw(&mut worker(), s);
        assert!(
            ids == fresh_ids,
            "scene {i}: the IDs differ from a fresh worker's"
        );
        assert!(
            png == fresh_png,
            "scene {i}: the PNG differs from a fresh worker's"
        );
    }
}

/// The pixels of `ids` drawn as an edge of `scene`'s first body more than 2.5 pixels from
/// where that edge projects.
fn edge_pixels_off(scene: &RenderScene, ids: &[PixelId]) -> Vec<(i64, i64, u32)> {
    let w = i64::from(scene.width);
    (0..ids.len())
        .filter(|&i| ids[i][0] == 1 && ids[i][1] & EDGE_BIT != 0)
        .map(|i| {
            let i = i64::try_from(i).unwrap();
            (
                i % w,
                i / w,
                ids[usize::try_from(i).unwrap()][1] & !EDGE_BIT,
            )
        })
        .filter(|&(col, row, edge)| !near_edge(scene, edge, col, row))
        .collect()
}

#[test]
fn an_oblique_orthographic_view_with_its_target_a_kilometre_along_the_ray_draws_its_edges_where_they_are()
 {
    // Was a break (the edges about the camera's point; M4.6 drew the faces so too): the
    // proptest's smallest case. A slab 1 × 1 × 1e6 from the origin, seen level along
    // (1, −1.164, 0), the target 1e6 mm along the ray from its corner and 0.73 mm aside, at
    // the least height the reach allows (14.14 mm over 2048 rows, a pixel 6.9e-3 mm). Each
    // edge point's offset from the target, some (6.5e5, 7.6e5, up to 1e6), is f32 to 1/16
    // mm and rotated in f32: the edges along the slab are drawn several pixels from where
    // they project, rows where the ray cast by hand sees nothing within 2.5 pixels.
    let toward = unit([1.0, -1.163_696_789_679_518, 0.0]);
    let target = along(
        along([0.0; 3], toward, 1e6),
        [0.0, 0.730_957_118_661_884, 0.0],
        1.0,
    );
    let mut scene = RenderScene {
        bodies: vec![body(1, box_mesh([0.0; 3], [1.0, 1.0, 1e6]))],
        camera: Camera {
            projection: Projection::Orthographic { view_height: 1.0 },
            eye: along(target, toward, -10.0),
            target,
            up: [0.349_876_935_034_664_55, 0.0, 0.0],
        },
        width: 40,
        height: 2048,
        ids: true,
    };
    scene.camera.projection = Projection::Orthographic {
        view_height: scene.reach(target) * MIN_VIEW_HEIGHT_OF_REACH * 1.0001,
    };
    let mut w = worker();
    // With the target by the slab's corner (the same view, as tall as the far target's
    // reach asks), the edges are where they project.
    let mut near = scene.clone();
    near.camera.target = along(target, toward, -1e6);
    near.camera.eye = along(near.camera.target, toward, -10.0);
    let (_, ids) = draw(&mut w, &near);
    let off = edge_pixels_off(&near, &ids);
    assert!(
        off.is_empty(),
        "the target near: {:?}",
        &off[..off.len().min(6)]
    );
    let (_, ids) = draw(&mut w, &scene);
    let drawn = ids.iter().filter(|id| id[1] & EDGE_BIT != 0).count();
    assert!(drawn > 40, "only {drawn} edge pixels");
    let off = edge_pixels_off(&scene, &ids);
    assert!(
        off.is_empty(),
        "{} of {drawn} edge pixels more than 2.5 pixels from their edge's projection \
         (column, row, edge), the first {:?}",
        off.len(),
        &off[..off.len().min(6)]
    );
}
