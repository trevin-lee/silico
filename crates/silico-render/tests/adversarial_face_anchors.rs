//! Hostile scenes for the point each face is drawn about (`/break-it` on
//! crates/silico-render/src/render.rs at 2ec921e6: each face uploaded about its own box's
//! centre, `View::anchor` choosing per face and per view between that centre and the
//! camera's point by the corner's sec² and, orthographic, the target's distance against the
//! face's half diagonal; `settle` dropping a face's meshes about another point or in the
//! colour it isn't drawn in; and the edges built each view about the camera's point).
//!
//! What's drawn is checked against a projection or a ray cast by hand in f64 here, or
//! against a fresh worker's drawing byte for byte, never against a stored image.
//!
//! The scenes at the render check's limits are at the limits as the image's size sets them
//! (docs/design/m4.6-render.md, *A given camera*): f32 to 2^-22 of a point's distance from
//! the camera's point, at most an eighth of its pixel. An orthographic view at least `h ×
//! reach / 2^19` high ([`least`], by hand here), a perspective view's angle within its
//! size's range, and its points before the eye and at most `2^20 t / h` across the view for
//! their depth.

use std::cell::{Cell, RefCell};
use std::path::Path;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use silico_core::ErrorCode;
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Camera, EDGE_BIT, Generation, Mesh, Op, PixelId, Projection, RenderBody, RenderScene, Reply,
    field_of_view_range,
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

/// `scene` refused by its check and by `w`, naming `parameter`, with `words` in each message;
/// and `w` still draws.
fn refused(w: &mut Worker, scene: &RenderScene, parameter: &str, words: &str) {
    let e = scene.check().unwrap_err();
    assert_eq!(e.parameter, parameter, "{e:?}");
    assert!(e.message.contains(words), "{}", e.message);
    match w.call(Generation(1), Op::Render(Box::new(scene.clone())), WAIT) {
        Ok(reply) => panic!("drawn, not refused: {reply:?}"),
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
            assert_eq!(e.parameter(), Some(parameter), "{e:?}");
            assert!(e.message().contains(words), "{e:?}");
        }
    }
    let small = RenderScene {
        bodies: vec![body(1, box_mesh([0.0; 3], [1.0; 3]))],
        camera: Camera {
            projection: Projection::Orthographic { view_height: 2.0 },
            eye: [0.5, -10.0, 0.5],
            target: [0.5; 3],
            up: [0.0, 0.0, 1.0],
        },
        width: 8,
        height: 8,
        ids: true,
    };
    draw(w, &small);
}

/// The least an orthographic view `height` pixels high may be of bodies reaching `reach`
/// from its target, by the design's rule: `height × reach / 2^19`.
fn least(height: u32, reach: f64) -> f64 {
    f64::from(height) * reach / 524_288.0
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

/// The ray through the pixel at `(u, v)`: its origin and unit direction.
fn ray(scene: &RenderScene, u: f64, v: f64) -> ([f64; 3], [f64; 3]) {
    let c = &scene.camera;
    let (forward, right, up) = basis(c);
    let (w, h) = (f64::from(scene.width), f64::from(scene.height));
    let (x, y) = (2.0 * u / w - 1.0, 1.0 - 2.0 * v / h);
    match c.projection {
        Projection::Orthographic { view_height } => {
            let half = view_height / 2.0;
            let on = along(along(c.target, right, x * half * w / h), up, y * half);
            (along(on, forward, -1e7), forward)
        }
        Projection::Perspective { field_of_view } => {
            let t = libm::tan((field_of_view / 2.0).to_radians());
            (
                c.eye,
                unit(along(along(forward, right, x * t * w / h), up, y * t)),
            )
        }
    }
}

/// The face the ray through the pixel at `(u, v)` leaves the box at `low` of `size` by, and
/// how far it is from the face it enters by, along the ray.
fn exit(scene: &RenderScene, low: [f64; 3], size: [f64; 3], u: f64, v: f64) -> Option<(u32, f64)> {
    let (origin, dir) = ray(scene, u, v);
    let origin = sub(origin, low);
    let (mut near, mut far, mut face) = (f64::NEG_INFINITY, f64::INFINITY, 0);
    for k in 0..3 {
        if dir[k] == 0.0 {
            continue;
        }
        let (t0, t1) = ((0.0 - origin[k]) / dir[k], (size[k] - origin[k]) / dir[k]);
        let (enter, leave, side) = if t0 < t1 { (t0, t1, 1) } else { (t1, t0, 0) };
        near = near.max(enter);
        if leave < far {
            far = leave;
            face = 2 * u32::try_from(k).unwrap() + 1 + side;
        }
    }
    (near <= far).then_some((face, far - near))
}

/// The misses of [`cast_misses`] that aren't a face behind the one in front less than
/// `depth` behind it along the ray, where f32's depth can't tell them apart.
fn misses_past_depth(
    scene: &RenderScene,
    low: [f64; 3],
    size: [f64; 3],
    misses: Vec<Miss>,
    depth: f64,
) -> Vec<Miss> {
    misses
        .into_iter()
        .filter(|&((col, row), got, _)| {
            let centre = |n: i64| n as f64 + 0.5;
            !exit(scene, low, size, centre(col), centre(row))
                .is_some_and(|(face, behind)| got == [1, face] && behind < depth)
        })
        .collect()
}

/// How far from where its edge projects an edge's pixel may be, across and along the image:
/// an edge is drawn 2 pixels wide, so its pixels' centres are within a pixel of its line,
/// and at the check's limits f32 moves it by at most an eighth of one.
const EDGE_SLACK: f64 = 1.0;

/// Whether the centre of the pixel at `(col, row)` is within an eighth of a pixel of where
/// face `face` of the box at `low` of `size` projects (by hand, its four corners, every one
/// before a perspective eye): the precision the renderer holds a face to, which the casts,
/// half a pixel apart, can't see of a face seen so nearly edge on that it's a sliver
/// narrower than that (a slab's side from 0.008 mm off its plane, 0.006 pixels wide where
/// a cast found it drawn over a pixel's centre).
fn near_face(
    scene: &RenderScene,
    low: [f64; 3],
    size: [f64; 3],
    face: u32,
    col: i64,
    row: i64,
) -> bool {
    let Some(k) = face
        .checked_sub(1)
        .and_then(|k| usize::try_from(k).ok())
        .filter(|k| *k < 6)
    else {
        return false;
    };
    let (axis, side) = (k / 2, k % 2);
    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
    let corner = |i: f64, j: f64| {
        let mut p = low;
        p[axis] += if side == 1 { size[axis] } else { 0.0 };
        p[a] += size[a] * i;
        p[b] += size[b] * j;
        p
    };
    let corners = [
        corner(0.0, 0.0),
        corner(1.0, 0.0),
        corner(1.0, 1.0),
        corner(0.0, 1.0),
    ];
    if matches!(scene.camera.projection, Projection::Perspective { .. }) {
        let (forward, _, _) = basis(&scene.camera);
        if corners
            .iter()
            .any(|&p| dot(sub(p, scene.camera.eye), forward) <= 0.0)
        {
            return false;
        }
    }
    let q: Vec<(f64, f64)> = corners.iter().map(|&p| pixel(scene, p)).collect();
    let (x, y) = (col as f64 + 0.5, row as f64 + 0.5);
    // Inside the projected quad (convex: a face's corners in perspective too), or within an
    // eighth of a pixel of one of its sides.
    let sides: Vec<f64> = (0..4)
        .map(|i| {
            let ((ax, ay), (bx, by)) = (q[i], q[(i + 1) % 4]);
            (bx - ax) * (y - ay) - (by - ay) * (x - ax)
        })
        .collect();
    if sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0) {
        return true;
    }
    (0..4).any(|i| {
        let ((ax, ay), (bx, by)) = (q[i], q[(i + 1) % 4]);
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (ex, ey) = (ax + t * dx - x, ay + t * dy - y);
        (ex * ex + ey * ey).sqrt() <= 0.125
    })
}

/// Whether the pixel at `(col, row)` is within [`EDGE_SLACK`] of where the edge `edge` of
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
        (c - x).abs() <= EDGE_SLACK && (r - y).abs() <= EDGE_SLACK
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

/// The least height over 4096 rows of the rod's view from its farthest target, (7.81, −6e5,
/// 50), at 1.0001 times: by hand (mpmath), its far corner (1e6, 1, 0) is
/// √(999992.19² + 600001² + 50²) = 1166184.1975 mm off, so 4096 × that / 2^19 = 9110.8140
/// mm, and 1.0001 times it 9111.7251, a pixel 2.22454 mm.
const ROD_VIEW: f64 = 9_111.725_124_585_02;

/// The kilometre rod (1e6 × 1 × 100 from the origin) from the front, orthographic,
/// `view_height` over 4096 rows, 64 wide, the view's centre at x = 7.81, z = 50, its target
/// `y` along the view's ray (the image doesn't depend on it: only the eye's direction
/// counts).
fn rod_from_the_front(y: f64, view_height: f64) -> RenderScene {
    let target = [7.81, y, 50.0];
    RenderScene {
        bodies: vec![body(1, box_mesh([0.0; 3], [1e6, 1.0, 100.0]))],
        camera: Camera {
            projection: Projection::Orthographic { view_height },
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
    // Was a break (#332, at M4.6's floor of 1e-5 of the reach): a target 6e5 in front of
    // (or behind) the rod's front face is farther from the face's box than its half diagonal
    // (5e5), so `View::anchor` keeps the face about its centre (5e5, 0, 50), and at 11.67 mm
    // over 4096 rows f32 about it missed (5e5 × 2^-22 = 0.12 mm against an eighth of a
    // 2.85e-3 mm pixel): the face's end was drawn at the view's centre. That view is now
    // refused, as f32 can't draw the reach so close; at the least height the reach allows,
    // a pixel is 2.2245 mm, and f32 about the face's centre is within 0.054 of one.
    //
    // By hand: the end (x = 0) is 7.81 mm left of the centre, at column 32 − 7.81 / 2.22454
    // = 28.489; the edge there is 2 pixels wide, so from column 30 on, row 2048 (z = 48.9)
    // is the front face, and column 25 is the background.
    let mut w = worker();
    refused(
        &mut w,
        &rod_from_the_front(-6e5, 11.67),
        "camera.view_height",
        "is too small for an image 4096 pixels high",
    );
    for y in [0.0, -6e5, 6e5] {
        let scene = rod_from_the_front(y, ROD_VIEW);
        let (col, _) = pixel(&scene, [0.0, 0.0, 48.9]);
        assert!((col - 28.489).abs() < 0.001, "{col}");
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
    // (As above, at the least height the farthest target allows): every pixel clear of a
    // boundary is the same with the target on the rod's face and 6e5 along the ray either
    // way, as the design says of an orthographic view (only the eye's direction counts).
    let mut w = worker();
    let (_, want) = draw(&mut w, &rod_from_the_front(0.0, ROD_VIEW));
    for y in [-6e5, 6e5] {
        let (_, got) = draw(&mut w, &rod_from_the_front(y, ROD_VIEW));
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
/// at 0.001°, M4.6's narrowest, a pixel there is 99999 × 2 tan(0.0005°) / 4096 = 4.26e-4
/// mm, and a 0.1 mm box about 235 pixels across.
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

/// The narrowest a view 384 by 4096 may be, by hand (mpmath): the smaller root of
/// `4096 (1 + (384/4096)²) t² − 2^20 t + 4096 = 0`, as 2 atan(t), 0.447627891064195°.
const NARROWEST_AT_4096: f64 = 0.447_627_891_064_195_1;

/// [`far_narrow_view`] at the narrowest angle 384 by 4096 allows, its box as many pixels
/// across as 0.1 mm at 0.001° (100 × the angle, 44.76 mm): a pixel at the box 0.1917 mm.
fn far_narrowest_view() -> RenderScene {
    let (narrowest, _) = field_of_view_range(384, 4096);
    assert!(
        (narrowest - NARROWEST_AT_4096).abs() <= 1e-15,
        "{narrowest}"
    );
    far_narrow_view(narrowest, 100.0 * narrowest)
}

#[test]
fn at_the_narrowest_field_of_view_the_edges_lie_where_the_box_projects_them() {
    // Was a break, at M4.6's narrowest (0.001°, any size): the edges are built about the
    // eye, each point's offset from it (some 3e4 to 7e4 mm on each axis) in f32, rounded to
    // 1/256 or 1/128 mm on each, and rotated in f32, so the box's corners moved 2 to 16
    // pixels across the view (a pixel 4.26e-4 mm), and the ID buffer gave an edge across a
    // face. At 4096 rows that angle is refused now, naming the angles this size draws; at
    // the narrowest of them (0.4476°) a pixel is 0.19 mm and f32 within an eighth of it, so
    // every tenth of every edge in view is drawn within a pixel of where it projects.
    let mut w = worker();
    refused(
        &mut w,
        &far_narrow_view(1e-3, 0.1),
        "camera.field_of_view",
        "camera.field_of_view 0.001 must be from 0.447628 to 179.548437 degrees for an image 384 by 4096 pixels",
    );
    let scene = far_narrowest_view();
    let (_, ids) = draw(&mut w, &scene);
    let off = edges_off_their_projection(&scene, &ids, &SEEN_FROM_BELOW);
    // Where each edge missed is drawn instead: its nearest pixel to the projection, in
    // pixels (none if it isn't drawn at all, as hidden behind a face).
    let nearest: Vec<(u32, u32, Option<f64>)> = off
        .iter()
        .map(|&(edge, k, col, row)| {
            let d = (0..ids.len())
                .filter(|&i| ids[i] == [1, EDGE_BIT | edge])
                .map(|i| {
                    let (c, r) = ((i % 384) as f64 + 0.5, (i / 384) as f64 + 0.5);
                    ((c - col) * (c - col) + (r - row) * (r - row)).sqrt()
                })
                .fold(None, |m: Option<f64>, d| Some(m.map_or(d, |m| m.min(d))));
            (edge, k, d)
        })
        .collect();
    assert!(
        off.is_empty(),
        "{} tenths of the 9 edges in view drawn off their projection by hand (edge, tenth, \
         nearest pixel of its ID, in pixels): {:?}",
        off.len(),
        nearest
    );
    for hidden in [3, 4, 12] {
        assert!(!ids.contains(&[1, EDGE_BIT | hidden]), "Edge{hidden} drawn");
    }
}

#[test]
fn at_the_narrowest_field_of_view_no_face_behind_the_box_is_drawn() {
    // Was a break (f32's depth, from M4.6), at 0.001°: from (+x, +y, −z), a box shows its
    // faces 2, 4, and 5 alone, and every other pixel clear of a boundary is what a ray cast
    // by hand sees. Drawn 99,999 mm off at 0.001°, the depths were f32 at 1e5 mm (a step of
    // 1/128 mm) while a pixel is 4.26e-4 mm across, so Face6 (z high, behind) was drawn over
    // Face4 some 14 pixels along their edge. At the narrowest angle 4096 rows allow, a pixel
    // is 0.19 mm, 25 depth steps: no face behind is drawn, and every clear pixel is the cast's.
    let mut w = worker();
    let scene = far_narrowest_view();
    let size = scene.bodies[0]
        .mesh
        .positions
        .iter()
        .fold(0.0f64, |m, p| m.max(p[0]));
    let (_, ids) = draw(&mut w, &scene);
    let behind: usize = [1, 3, 6]
        .iter()
        .map(|f| ids.iter().filter(|id| **id == [1, *f]).count())
        .sum();
    let (wrong, on) = cast_misses(&scene, [0.0; 3], [size; 3], &ids, 2);
    assert!(on > 5_000, "the box is checked at only {on} pixels");
    assert!(
        behind == 0 && wrong.is_empty(),
        "{behind} pixels of faces 1, 3, and 6, behind the box; {} pixels clear of a boundary \
         aren't what a ray cast sees, the first {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(5)]
    );
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
// The near plane: half the nearest point's depth from the eye.

#[test]
fn a_close_look_at_a_small_box_with_a_body_a_kilometre_ahead_still_sees_the_box() {
    // Was a break (from M4.6, in `View::of`): the near plane was a millionth of the farthest
    // point's distance from the eye, so a second body 1e6 mm off put it 1 mm out, past the
    // box's front face 0.5 mm in front of the eye: the face was clipped whole. It's half the
    // nearest point's depth now, 0.25 mm. By hand, the eye 0.5 mm in front of the front
    // face's centre looking at it, 60° across: the whole image is the front face (it spans
    // ±0.5 mm, the view at 0.5 mm ±0.29 mm high), alone or with a body 1e6 mm ahead (behind
    // the box) or 1e6 mm aside.
    //
    // A body 1e6 mm aside at 0.5 to 1.5 mm deep is 1e6 / 0.5 = 2e6 times as far across the
    // view as along it, past the 2^20 tan(30°) / 48 = 12612 f32 draws to an eighth of a
    // pixel over 48 rows: refused, naming the eye. The core leaves it out (it's outside the
    // view, `sees`), so a render draws the box alone.
    let mut w = worker();
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 60.0,
        },
        eye: [0.5, -0.5, 0.5],
        target: [0.5, 0.0, 0.5],
        up: [0.0, 0.0, 1.0],
    };
    let scene = |bodies: Vec<RenderBody>| RenderScene {
        bodies,
        camera: camera.clone(),
        width: 64,
        height: 48,
        ids: true,
    };
    let the_box = || body(1, box_mesh([0.0; 3], [1.0; 3]));
    let (_, alone) = draw(&mut w, &scene(vec![the_box()]));
    assert!(alone.iter().all(|id| *id == FRONT), "alone: {:?}", alone[0]);
    let ahead = body(2, box_mesh([0.0, 1e6, 0.0], [1.0; 3]));
    let (_, ids) = draw(&mut w, &scene(vec![the_box(), ahead]));
    let wrong = ids.iter().filter(|id| **id != FRONT).count();
    assert_eq!(
        wrong, 0,
        "with a body 1e6 ahead: {wrong} pixels not the front face"
    );
    let aside = [[1e6, 0.0, 0.0], [1e6 + 1.0, 1.0, 1.0]];
    refused(
        &mut w,
        &scene(vec![the_box(), body(2, box_mesh(aside[0], [1.0; 3]))]),
        "camera.eye",
        "body 2 is 2000001.",
    );
    assert_eq!(
        silico_core::views::sees(&camera, 64, 48, aside),
        Some(false)
    );
}

// ---------------------------------------------------------------------------------------
// The held meshes, as faces cross between their centres and the camera's point.

#[test]
fn a_sequence_that_moves_faces_between_their_centres_and_the_eye_draws_as_a_fresh_worker_does() {
    // The rod seen 40 mm off at 120° (its long faces about the eye, re-uploaded each view:
    // 5e5 / 40 = 12500 times as large as near, past the 2^20 tan(60°) / (64 × 7) = 4054
    // their centres hold to an eighth of a pixel at the corner, sec² 7; its ends about their
    // centres), then from 5e5 mm (all about their centres), then close again, with
    // highlights turned on and off as faces move between the two points: each scene's PNG
    // and ID buffer from one worker holding the meshes before it against a fresh worker's,
    // byte for byte. At 40 mm its far end is 1e6 / 40 = 25000 times as far across as along,
    // under the 2^20 tan(60°) / 64 = 28378 f32 draws.
    let rod = box_mesh([0.0; 3], [1e6, 20.0, 10.0]);
    let view = |x: f64, distance: f64, faces: Vec<u32>, edges: Vec<u32>| RenderScene {
        bodies: vec![RenderBody {
            highlight_faces: faces,
            highlight_edges: edges,
            ..body(1, rod.clone())
        }],
        camera: Camera {
            projection: Projection::Perspective {
                field_of_view: 120.0,
            },
            eye: [x, -distance, 5.0],
            target: [x, 0.0, 5.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 64,
        height: 64,
        ids: true,
    };
    let scenes = [
        view(0.01, 40.0, vec![], vec![]),
        view(0.01, 40.0, vec![2], vec![3]),
        view(0.012, 40.0, vec![2, 3], vec![]),
        view(0.012, 5e5, vec![2], vec![]),
        view(0.012, 5e5, vec![], vec![0, 3]),
        view(0.011, 40.0, vec![], vec![0, 3]),
        view(0.011, 40.0, vec![0, 1, 2, 3, 4, 5], vec![]),
        view(0.01, 40.0, vec![], vec![]),
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

#[test]
fn a_box_at_each_of_the_checks_limits_draws_as_a_fresh_worker_does() {
    // The bench's box (40 × 20 × 10 from the origin), 64 by 64, at each limit the check
    // takes, in turn on one worker, each against a fresh worker's drawing byte for byte:
    // orthographic at its least height from its corner, 64 √2100 / 2^19 (its far corner is
    // √2100 off); perspective at the narrowest and the widest angle 64 by 64 allows (by hand,
    // 0.006994° and 179.986012°), from 1e5 mm and from a nanometre before its front face,
    // the least a point may be before the eye; and from just past where the front face's
    // far corner, √490 mm off the axis through (21, 0, 7), is 2^20 tan(30°) / 64 = 9459.31
    // times as far across the view as along it, at 60°.
    let the_box = box_mesh([0.0; 3], [40.0, 20.0, 10.0]);
    let (narrowest, widest) = field_of_view_range(64, 64);
    assert!(
        (narrowest - 0.006_994_113_753_518_231).abs() <= 1e-15,
        "{narrowest}"
    );
    assert!((widest - 179.986_011_772_545_07).abs() <= 1e-12, "{widest}");
    let from_front = |projection: Projection, distance: f64| RenderScene {
        bodies: vec![body(1, the_box.clone())],
        camera: Camera {
            projection,
            eye: [21.0, -distance, 7.0],
            target: [21.0, 10.0, 7.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 64,
        height: 64,
        ids: true,
    };
    let across = 490f64.sqrt() / 9_459.306_810_402_83 * (1.0 + 1e-9);
    let scenes = [
        RenderScene {
            camera: Camera {
                projection: Projection::Orthographic {
                    view_height: least(64, 2100f64.sqrt()),
                },
                eye: [-10.0, -10.0, 10.0],
                target: [0.0; 3],
                up: [0.0, 0.0, 1.0],
            },
            ..from_front(Projection::Orthographic { view_height: 1.0 }, 1.0)
        },
        from_front(
            Projection::Perspective {
                field_of_view: narrowest,
            },
            1e5,
        ),
        from_front(
            Projection::Perspective {
                field_of_view: widest,
            },
            1e-6,
        ),
        from_front(
            Projection::Perspective {
                field_of_view: 60.0,
            },
            across,
        ),
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
    // Each a step past its limit is refused.
    let mut past = scenes.clone();
    past[0].camera.projection = Projection::Orthographic {
        view_height: least(64, 2100f64.sqrt()).next_down(),
    };
    past[1].camera.projection = Projection::Perspective {
        field_of_view: narrowest.next_down(),
    };
    past[2].camera.eye[1] = -9.99e-7;
    past[3].camera.eye[1] = -across * (1.0 - 2e-9);
    for (i, (s, parameter)) in past
        .iter()
        .zip([
            "camera.view_height",
            "camera.field_of_view",
            "camera.eye",
            "camera.eye",
        ])
        .enumerate()
    {
        assert_eq!(s.check().unwrap_err().parameter, parameter, "scene {i}");
    }
}

// ---------------------------------------------------------------------------------------
// Random long slabs seen close, perspective, and orthographic at their least zoom with the
// target anywhere along the ray, two views each from one worker, every clear pixel against
// a ray cast by hand.

/// A slab: one side long (up to the document's largest length), two short, at an offset
/// biased to where f32 runs out. Short sides up to 2 m too: at the least orthographic zoom
/// a kilometre slab's reach allows, a pixel is some 2 mm, so a slab a few millimetres across
/// is under a pixel, and only wider ones show the cast anything clear of a boundary.
fn slab() -> impl Strategy<Value = ([f64; 3], [f64; 3])> {
    let long = prop_oneof![Just(1e6), Just(1e5), 1e3..1e6f64, 10.0..1e3f64];
    let short = || prop_oneof![1 => Just(1.0), 1 => 0.5..20.0f64, 2 => 20.0..2e4f64];
    let offset = prop_oneof![Just(0.0), Just(1e9), Just(-1e9), -1e6..1e6f64];
    (
        0..3usize,
        long,
        short(),
        short(),
        prop::array::uniform3(offset),
    )
        .prop_map(|(axis, long, a, b, low)| {
            let mut size = [a, b, a];
            size[axis] = long;
            size[(axis + 1) % 3] = a;
            size[(axis + 2) % 3] = b;
            (low, size)
        })
}

/// A place along a side, as a fraction: its ends and next to them, its middle, or anywhere.
fn fraction() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(1e-6),
        Just(0.5),
        Just(1.0 - 1e-6),
        Just(1.0),
        0.0..1.0f64
    ]
}

/// How a slab is seen: from which of its faces, where on it, how far, how tilted, and the
/// projection's numbers; and a second view's move.
#[derive(Debug, Clone)]
struct Look {
    face: usize,
    u: f64,
    v: f64,
    distance: f64,
    tilt: [f64; 2],
    up: [f64; 3],
    perspective: bool,
    fov: f64,
    zoom: f64,
    along: f64,
    tall: bool,
    step: [f64; 3],
}

fn look() -> impl Strategy<Value = Look> {
    (
        (
            0..6usize,
            fraction(),
            fraction(),
            // Close looks twice as often as far ones, and those at most a metre off: the
            // check refuses close perspective looks at a kilometre slab, and farther ones at
            // a wide angle see a slab under a pixel.
            prop_oneof![1 => Just(1e-2), 2 => 1e-2..50.0f64, 1 => 50.0..1e3f64],
            prop::array::uniform2(prop_oneof![Just(0.0), -1.5..1.5f64]),
            prop::array::uniform3(-1.0..1.0f64),
        ),
        (
            any::<bool>(),
            prop_oneof![Just(0.5), 0.5..120.0f64],
            // From the least zoom the image's height allows (`h × reach / 2^19`, up to 391
            // times M4.6's 1e-5 of the reach) to a hundred times it.
            prop_oneof![Just(1.0001), 1.0..10.0f64, 10.0..100.0f64],
            prop_oneof![Just(0.0), Just(-1.0), Just(1.0), -1.0..1.0f64],
            any::<bool>(),
            prop::array::uniform3(prop_oneof![Just(0.0), -1e-2..1e-2f64, -1.0..1.0f64]),
        ),
    )
        .prop_map(
            |((face, u, v, distance, tilt, up), (perspective, fov, zoom, along, tall, step))| {
                Look {
                    face,
                    u,
                    v,
                    distance,
                    tilt,
                    up,
                    perspective,
                    fov,
                    zoom,
                    along,
                    tall,
                    step,
                }
            },
        )
}

/// The scene of the slab at `low` of `size` from `look`, its eye moved by `step` (the
/// second view), orthographic at `look.zoom` times the least height its reach allows (`h ×
/// reach / 2^19`), or none where the numbers make no view the check takes (an angle past the
/// size's, or a perspective eye too near the slab for how far across the view it reaches).
fn scene_of(low: [f64; 3], size: [f64; 3], look: &Look, step: [f64; 3]) -> Option<RenderScene> {
    let axis = look.face / 2;
    let high = look.face % 2 == 1;
    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
    let mut point = low;
    point[axis] += if high { size[axis] } else { 0.0 };
    point[a] += size[a] * look.u;
    point[b] += size[b] * look.v;
    let mut outward = [0.0; 3];
    outward[axis] = if high { 1.0 } else { -1.0 };
    let mut toward = outward.map(|c| -c);
    toward[a] += look.tilt[0];
    toward[b] += look.tilt[1];
    let toward = unit(toward);
    let (eye, target, projection) = if look.perspective {
        let eye = along(along(point, outward, look.distance), step, 1.0);
        let target = along(eye, toward, 10.0);
        (
            eye,
            target,
            Projection::Perspective {
                field_of_view: look.fov,
            },
        )
    } else {
        // The target on the face's point, then anywhere along the ray up to the slab's
        // length either way.
        let longest = size.iter().fold(0.0f64, |m, s| m.max(*s));
        let target = along(along(point, toward, look.along * longest), step, 1.0);
        let eye = along(target, toward, -10.0);
        (eye, target, Projection::Orthographic { view_height: 1.0 })
    };
    let (width, height) = if look.tall { (40, 2048) } else { (64, 48) };
    let mut scene = RenderScene {
        bodies: vec![body(1, box_mesh(low, size))],
        camera: Camera {
            projection,
            eye,
            target,
            up: look.up,
        },
        width,
        height,
        ids: true,
    };
    if scene.camera.check(width, height).is_err() {
        return None;
    }
    if !look.perspective {
        let reach = scene.reach(target);
        scene.camera.projection = Projection::Orthographic {
            view_height: (least(height, reach) * look.zoom).max(1e-6),
        };
    }
    scene.check().is_ok().then_some(scene)
}

/// [`random_close_looks_at_long_slabs_draw_every_clear_pixel_as_a_ray_cast_by_hand_sees_it`],
/// in perspective alone if `perspective`.
fn random_close_looks(perspective: bool) {
    let w = RefCell::new(worker());
    let mut runner = TestRunner::new_with_rng(
        Config {
            // The check refuses most close perspective looks at a kilometre slab (too far
            // across the view for their depth), so a perspective run takes more cases for
            // as many views drawn.
            cases: if perspective { 2400 } else { 1000 },
            failure_persistence: None,
            // Each shrink draws again: a few dozen find the smallest case well enough.
            max_shrink_iters: 64,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let drawn = Cell::new(0usize);
    let seen = Cell::new(0usize);
    runner
        .run(&(slab(), look()), |((low, size), mut look)| {
            look.perspective |= perspective;
            for (k, step) in [[0.0; 3], look.step].into_iter().enumerate() {
                let Some(scene) = scene_of(low, size, &look, step) else {
                    continue;
                };
                let (_, ids) = draw(&mut w.borrow_mut(), &scene);
                drawn.set(drawn.get() + 1);
                let (wrong, on) = cast_misses(&scene, low, size, &ids, 1);
                // f32's depth over the scene's depth (orthographic: the camera its reach
                // and more behind the target; perspective: positions from the eye) can't
                // tell a face from one this near behind it: that's
                // `a_long_slab_seen_from_its_end_...`'s break, not the anchor's.
                let from = match scene.camera.projection {
                    Projection::Orthographic { .. } => scene.camera.target,
                    Projection::Perspective { .. } => scene.camera.eye,
                };
                let depth = 4.0 * scene.reach(from) * f64::from(f32::EPSILON);
                // The edges, about the camera's point, are off on their own (the tests of
                // the edges above): only the faces are held to the cast here.
                // And a face drawn over a pixel whose centre is within an eighth of a pixel
                // of where it projects is where it is, though no cast saw it there.
                let wrong: Vec<_> = misses_past_depth(&scene, low, size, wrong, depth)
                    .into_iter()
                    .filter(|&(_, got, _)| got[1] & EDGE_BIT == 0)
                    .filter(|&((col, row), got, _)| {
                        !(got[0] == 1 && near_face(&scene, low, size, got[1], col, row))
                    })
                    .collect();
                if on > 0 {
                    seen.set(seen.get() + 1);
                }
                prop_assert!(
                    wrong.is_empty(),
                    "view {k} of the slab at {low:?} of {size:?}, {}x{}: {:?}: {} pixels \
                     clear of a boundary differ (at, got, want), the first {:?}",
                    scene.width,
                    scene.height,
                    scene.camera,
                    wrong.len(),
                    &wrong[..wrong.len().min(5)]
                );
            }
            Ok(())
        })
        .unwrap();
    assert!(
        drawn.get() > 1200,
        "only {} views drawn, {} of them seeing the slab clear of its boundaries",
        drawn.get(),
        seen.get()
    );
    assert!(
        seen.get() * 2 > drawn.get(),
        "the slab seen clear of its boundaries in only {} of {} views",
        seen.get(),
        drawn.get()
    );
}

#[test]
fn random_close_looks_at_long_slabs_draw_every_clear_pixel_as_a_ray_cast_by_hand_sees_it() {
    // Was a break, at M4.6's floor of 1e-5 of the reach (the orthographic anchor, as in
    // `an_orthographic_target_far_along_the_ray_from_a_long_face_draws_its_end_where_it_is`):
    // its smallest case was the slab 1 × 1 × 1e6 from the origin seen level along
    // (1, −1.164, 0), the target 1e6 mm along the ray toward the eye from its corner and
    // 0.73 mm aside, at that floor (14.14 mm over 2048 rows): Face1 was drawn about its
    // centre some 3 pixels off. Each orthographic view is at the floor the image's height
    // sets now, `h × reach / 2^19`, or above it.
    random_close_looks(false);
}

#[test]
fn random_close_perspective_looks_at_long_slabs_draw_every_clear_face_pixel_as_a_ray_cast_sees_it()
{
    // The perspective half alone, so a break in the orthographic half doesn't end the run
    // before it: each face about its centre or the eye by the corner's sec².
    //
    // Was a break (the rasterizer, on the eye's path from M4.6): its smallest case was
    // `a_long_face_seen_close_from_before_its_middle_at_a_narrow_angle_is_drawn_whole`'s,
    // which the check refuses now (a point too far across the view for its depth): the
    // views `scene_of` keeps are those the check takes.
    random_close_looks(true);
}

/// The slab's length in the proptest's smallest failing case.
const SLAB: f64 = 679_401.368_106_674_8;

/// The proptest's smallest failing case: the slab 679401.37 long from the origin, `side` by
/// `side` across (1 in the case), seen orthographic from its end, the target its corner (0,
/// 0, 0), from (−x, below) 20° up ((1, 0, 0.364) along the view), up nearly −z, `view_height`
/// over 2048 rows; with its top (Face6) as `box_mesh` meshes it, two triangles 679 m long, or
/// as a strip of `pieces` along x.
fn slab_seen_from_its_end(pieces: u32, side: f64, view_height: f64) -> RenderScene {
    let length = SLAB;
    let mut mesh = box_mesh([0.0; 3], [length, side, side]);
    if pieces > 1 {
        mesh.triangles.truncate(10);
        mesh.face_triangles[5] = 2 * pieces;
        for k in 0..pieces {
            let x = |k: u32| length * f64::from(k) / f64::from(pieces);
            let base = u32::try_from(mesh.positions.len()).unwrap();
            mesh.positions.extend([
                [x(k), 0.0, side],
                [x(k + 1), 0.0, side],
                [x(k + 1), side, side],
                [x(k), side, side],
            ]);
            mesh.normals.extend([[0.0, 0.0, 1.0]; 4]);
            mesh.triangles
                .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        }
    }
    let toward = unit([1.0, 0.0, 0.363_841_726_542_431_6]);
    RenderScene {
        bodies: vec![body(1, mesh)],
        camera: Camera {
            projection: Projection::Orthographic { view_height },
            eye: along([0.0; 3], toward, -10.0),
            target: [0.0; 3],
            up: [0.0, 0.0, -0.038_032_593_566_032_88],
        },
        width: 40,
        height: 2048,
        ids: true,
    }
}

#[test]
fn a_long_slab_seen_from_its_end_at_the_least_zoom_shows_its_end_not_its_hidden_top() {
    // Was a break (f32's depth, from M4.6: the camera sits the scene's reach and more
    // behind the target): from below its end, the slab shows its end (Face1) and its bottom
    // (Face5); its top (Face6) is behind both. At M4.6's floor, 1e-5 of the reach (6.795 mm
    // over 2048 rows, 1.0001 times it), a pixel was 3.3e-3 mm and every depth f32 to 1/16
    // mm, so the top showed through the end in the 12 rows above their shared edge. That
    // view is refused now, naming the least this height allows: by hand, 2048 ×
    // √(679401.3681066748² + 2) / 2^19 = 2653.911594172448 mm.
    //
    // At 1.0001 times the least (the slab 100 mm across, so its end is some 70 rows by 20
    // columns: at 1 mm it would be under a pixel; by hand, 2048 × √(679401.37² + 2 × 100²) /
    // 2^19 = 2653.9116516622), a pixel is 1.3 mm, the depth a tenth of one: every pixel clear
    // of a boundary is what the cast sees, the top two triangles 679 m long or a strip of
    // 1000.
    let mut w = worker();
    refused(
        &mut w,
        &slab_seen_from_its_end(1, 1.0, 6.794_693_082_449_575),
        "camera.view_height",
        "at least 2653.911595 mm",
    );
    let mut misses = vec![];
    for pieces in [1000, 1] {
        let scene = slab_seen_from_its_end(pieces, 100.0, 2_653.911_651_662_166_4 * 1.0001);
        let (_, ids) = draw(&mut w, &scene);
        let (wrong, on) = cast_misses(&scene, [0.0; 3], [SLAB, 100.0, 100.0], &ids, 1);
        assert!(on > 1_000, "{pieces} pieces: {on}");
        let end = ids.iter().filter(|id| **id == [1, 1]).count();
        assert!(end > 500, "{pieces} pieces: the end is {end} pixels");
        if !wrong.is_empty() {
            misses.push((pieces, wrong.len(), wrong[0]));
        }
    }
    assert!(
        misses.is_empty(),
        "the top in pieces: how many pixels clear of a boundary differ, and the first (at, \
         got, want): {misses:?}"
    );
}

/// The pixels of `ids` drawn as an edge of `scene`'s first body more than [`EDGE_SLACK`]
/// from where that edge projects.
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
    // Was a break (#332, at M4.6's floor; the edges about the camera's point): the
    // proptest's smallest case. A slab 1 × 1 × 1e6 from the origin, seen level along
    // (1, −1.164, 0), the target 1e6 mm along the ray from its corner and 0.73 mm aside, at
    // 1e-5 of the reach (14.14 mm over 2048 rows, a pixel 6.9e-3 mm): each edge point's
    // offset from the target, some (6.5e5, 7.6e5, up to 1e6), is f32 to 1/16 mm, and the
    // edges along the slab were drawn several pixels from where they project. That view is
    // refused now; at 1.0001 times the least this height allows, `2048 × reach / 2^19`, a
    // pixel is 2.2 mm and f32 within an eighth of it, so every edge pixel is within a pixel
    // of its edge's projection (the 2-pixel line's half width). The slab is 10 × 10 across
    // here, so its edges along it are some 4 pixels apart, not under one.
    let toward = unit([1.0, -1.163_696_789_679_518, 0.0]);
    let target = along(
        along([0.0; 3], toward, 1e6),
        [0.0, 0.730_957_118_661_884, 0.0],
        1.0,
    );
    let mut scene = RenderScene {
        bodies: vec![body(1, box_mesh([0.0; 3], [10.0, 10.0, 1e6]))],
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
    let reach = scene.reach(target);
    let mut w = worker();
    scene.camera.projection = Projection::Orthographic {
        view_height: reach * 1e-5 * 1.0001,
    };
    refused(
        &mut w,
        &scene,
        "camera.view_height",
        "is too small for an image 2048 pixels high",
    );
    scene.camera.projection = Projection::Orthographic {
        view_height: least(2048, reach) * 1.0001,
    };
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
    // Two edges along the slab at least, across the image's 40 columns.
    assert!(drawn > 80, "only {drawn} edge pixels");
    let off = edge_pixels_off(&scene, &ids);
    assert!(
        off.is_empty(),
        "{} of {drawn} edge pixels more than a pixel from their edge's projection \
         (column, row, edge), the first {:?}",
        off.len(),
        &off[..off.len().min(6)]
    );
}

/// The slab's length in the perspective proptest's smallest case.
const BOTTOM_SLAB: f64 = 636_410.347_555_590_1;

/// The perspective proptest's smallest case: the slab 636410.35 long from the origin, `width`
/// across y (1 in the case) and 1 high, its bottom (Face5) as `box_mesh` meshes it, or as a
/// strip of `pieces` along x; seen from `distance` below its bottom's centre looking up, up
/// +y, 40 by 2048, with `projection`.
fn slab_seen_from_below(
    pieces: u32,
    width: f64,
    distance: f64,
    projection: Projection,
) -> RenderScene {
    let length = BOTTOM_SLAB;
    let mut mesh = box_mesh([0.0; 3], [length, width, 1.0]);
    if pieces > 1 {
        let top: Vec<[u32; 3]> = mesh.triangles.split_off(8).split_off(2);
        mesh.face_triangles[4] = 2 * pieces;
        for k in 0..pieces {
            let x = |k: u32| length * f64::from(k) / f64::from(pieces);
            let base = u32::try_from(mesh.positions.len()).unwrap();
            mesh.positions.extend([
                [x(k), 0.0, 0.0],
                [x(k), width, 0.0],
                [x(k + 1), width, 0.0],
                [x(k + 1), 0.0, 0.0],
            ]);
            mesh.normals.extend([[0.0, 0.0, -1.0]; 4]);
            mesh.triangles
                .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        }
        mesh.triangles.extend(top);
    }
    let eye = [length / 2.0, width / 2.0, -distance];
    RenderScene {
        bodies: vec![body(1, mesh)],
        camera: Camera {
            projection,
            eye,
            target: [eye[0], eye[1], eye[2] + 10.0],
            up: [0.0, 0.665_472_384_913_471_3, 0.0],
        },
        width: 40,
        height: 2048,
        ids: true,
    }
}

#[test]
fn a_long_face_seen_close_from_before_its_middle_at_a_narrow_angle_is_drawn_whole() {
    // Was a break (the rasterizer, on the eye's path from M4.6): from 48.37 mm below the
    // slab's bottom at 0.5°, the bottom fills the view, but half the image was background:
    // its two triangles' far corners, 318 m off at 48 mm deep, land some 1e8 pixels off the
    // image, and the clipper's f32 slides a guard-band cut by 2^-24 of that. That view is
    // refused now, naming the eye: by hand, the far corners are √(318205.17² + 0.5²) /
    // 48.3746 = 6577.94 times as far across the view as along it, and 0.5° over 2048 rows
    // draws 2^20 tan(0.25°) / 2048 = 2.2340 times (mpmath).
    //
    // From 1.08 m below at 60°, the slab 500 mm across so it's some 820 of the 2048 rows
    // (a pixel 0.61 mm there), its far corners √(318205.17² + 250²) / 1080 = 294.63 times
    // as far across as along, under the 2^20 tan(30°) / 2048 = 295.60 drawn; or
    // orthographic, its target the bottom's centre, at 1.0001 times the least height the
    // reach allows (by hand, 2048 × √(318205.17² + 250² + 1) / 2^19 = 1242.989 mm): the bottom is drawn whole, its pixels
    // what the cast sees, as two triangles or a strip of 10,000.
    let mut w = worker();
    let narrow = Projection::Perspective { field_of_view: 0.5 };
    refused(
        &mut w,
        &slab_seen_from_below(1, 1.0, 48.374_580_540_203_375, narrow),
        "camera.eye",
        "body 1 is 6577.94",
    );
    let e = slab_seen_from_below(1, 1.0, 48.374_580_540_203_375, narrow)
        .check()
        .unwrap_err();
    assert!(
        e.message.contains("more than the 2.234035620199"),
        "{}",
        e.message
    );
    let mut misses = vec![];
    let views = [
        (
            Projection::Perspective {
                field_of_view: 60.0,
            },
            1080.0,
            30_000,
        ),
        (
            Projection::Orthographic {
                view_height: 1_242.989_343_696_985 * 1.0001,
            },
            10.0,
            15_000,
        ),
    ];
    for (projection, distance, least_on) in views {
        for pieces in [10_000, 1] {
            let scene = slab_seen_from_below(pieces, 500.0, distance, projection);
            let (_, ids) = draw(&mut w, &scene);
            let (wrong, on) = cast_misses(&scene, [0.0; 3], [BOTTOM_SLAB, 500.0, 1.0], &ids, 1);
            assert!(on > least_on, "{projection:?}, {pieces} pieces: {on}");
            if !wrong.is_empty() {
                misses.push((projection, pieces, wrong.len(), wrong[0]));
            }
        }
    }
    assert!(
        misses.is_empty(),
        "the bottom in pieces: how many pixels clear of a boundary differ, and the first (at, \
         got, want): {misses:?}"
    );
}
