//! break-it: hostile, degenerate, and boundary inputs for the mesh's normals and faces
//! (docs/ports/tessellation.md). Every expected normal is the surface's own, from its
//! closed form: a plane's is its axis (and its triangles' cross product), a cylinder's,
//! cone's, sphere's, and torus's come from the point and the shape's axis and radii. Where a
//! surface has no normal (an apex, a horn torus's center), the expected one is the average
//! of the node's triangles the port note documents, worked out from the mesh. Shapes with
//! rotations no Silico operation makes are made by rewriting the location a `translate`
//! writes into the B-rep, then reading it back, as a stored or hand-made B-rep would arrive;
//! most B-reps that `from_brep` must refuse (a mirror, a scale, a location that isn't
//! finite, a face with no surface) are made the same way, and the rest (a location's link
//! raised to a power, a face of only a triangulation) were written by OCCT
//! (tests/fixtures/brep/README.md). No platform trigonometry: rotations are exact (quarter
//! turns, 3-4-5 triangles, and integer quaternions).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/mesh_geometry.rs"]
mod geometry;
#[path = "common/kernel.rs"]
mod kernel;
#[path = "common/oracle.rs"]
mod oracle;

use geometry::{apart, cross, cylinder, dot, faces, norm, sub, wide};
use kernel::kernel;
use proptest::prelude::*;
use silico_core::kernel::ShapeData;
use silico_kernel::{
    Axis, BooleanOp, ChamferSize, KernelErrorKind, Mesh, Primitive, Profile, Shape, boolean,
    chamfer, fillet, from_brep, make, make_box, revolve, summarize, tessellate, to_brep, translate,
};
use silico_protocol::Blob;

fn unit(v: [f64; 3]) -> [f64; 3] {
    let n = norm(v);
    v.map(|c| c / n)
}

fn sphere(radius: f64) -> Shape {
    make(&Primitive::Sphere { radius }).unwrap()
}

fn cone(radius1: f64, radius2: f64, height: f64) -> Shape {
    make(&Primitive::Cone {
        radius1,
        radius2,
        height,
    })
    .unwrap()
}

fn torus(major_radius: f64, minor_radius: f64) -> Shape {
    make(&Primitive::Torus {
        major_radius,
        minor_radius,
    })
    .unwrap()
}

fn cut(base: &Shape, tool: &Shape) -> Shape {
    boolean(BooleanOp::Cut, base, tool).unwrap().shape
}

fn moved(shape: &Shape, offset: [f64; 3]) -> Shape {
    translate(shape, offset).unwrap().shape
}

/// The edges (numbered from 1, as the fillet takes them) with an end at `corner`.
fn edges_at(shape: &Shape, corner: [f64; 3]) -> Vec<u32> {
    let mesh = tessellate(shape, 0.1).unwrap();
    (1u32..)
        .zip(mesh.edges())
        .filter(|(_, e)| !e.is_empty() && (e[0] == corner || e[e.len() - 1] == corner))
        .map(|(n, _)| n)
        .collect()
}

/// A 20 × 20 × 20 box with a ball of radius 5 hollowed out of its middle, (10, 10, 10).
fn box_with_a_cavity() -> Shape {
    cut(
        &make_box(20.0, 20.0, 20.0).unwrap(),
        &moved(&sphere(5.0), [10.0, 10.0, 10.0]),
    )
}

/// A 20 × 20 × 10 box with a pointed cone cut up into it from below: base radius 5 at
/// z = -1, apex at (10, 10, 7), inside the box.
fn box_with_a_conical_pit() -> Shape {
    cut(
        &make_box(20.0, 20.0, 10.0).unwrap(),
        &moved(&cone(5.0, 0.0, 8.0), [10.0, 10.0, -1.0]),
    )
}

/// Every shape the kernel can build whose faces are of a different kind, or have a
/// singular point (a pole, an apex, a point where a torus touches its axis), or face the
/// other way (holes), or are blends and bevels.
fn shapes() -> Vec<(&'static str, Shape)> {
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let corner = edges_at(&b, [0.0; 3]);
    assert_eq!(corner.len(), 3, "{corner:?}");
    let triangle = Profile {
        points: vec![[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0, 0.0, 10.0]],
    };
    let z_axis = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };
    vec![
        ("box", make_box(10.0, 20.0, 30.0).unwrap()),
        ("cylinder", cylinder(5.0, 10.0)),
        ("sphere", sphere(5.0)),
        ("cone pointed at the top", cone(5.0, 0.0, 10.0)),
        ("cone pointed at the bottom", cone(0.0, 5.0, 10.0)),
        ("frustum", cone(5.0, 2.0, 10.0)),
        ("torus", torus(10.0, 2.0)),
        ("horn torus", torus(5.0, 5.0)),
        ("box with a cavity", box_with_a_cavity()),
        ("box with a conical pit", box_with_a_conical_pit()),
        ("filleted corner", fillet(&b, &corner, 2.0).unwrap().shape),
        (
            "chamfered corner",
            chamfer(&b, &corner, ChamferSize::Equal { distance: 2.0 }, false)
                .unwrap()
                .shape,
        ),
        (
            "revolved triangle (a cone)",
            revolve(&triangle, &z_axis, 360.0).unwrap().shape,
        ),
        (
            "revolved triangle, a quarter turn",
            revolve(&triangle, &z_axis, 90.0).unwrap().shape,
        ),
    ]
}

/// The deflection the shapes are meshed with.
const DEFLECTION: f64 = 0.05;

/// Checks what every mesh's normals must be, whatever the shape: unit vectors, and on the
/// side of each triangle (with an area) that the triangle faces, seen from where it winds
/// counterclockwise. On apexes, poles, and a torus's touching point too, where the surface
/// has no normal and OCCT falls back on the node's triangles.
fn assert_sound(what: &str, mesh: &Mesh) {
    assert_eq!(mesh.validate(), Ok(()), "{what}");
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let length = norm(wide(*n));
        assert!(
            (length - 1.0).abs() <= 1e-6,
            "{what}: |{n:?}| = {length} at {p:?}"
        );
    }
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        let facing = cross(sub(b, a), sub(c, a));
        // A sliver (three nodes on a line, as at a pole or along a cone's ruling) faces no
        // way: its cross product is rounding, and far from the origin that's 1e-11.
        let longest = [norm(sub(b, a)), norm(sub(c, b)), norm(sub(a, c))]
            .into_iter()
            .fold(0.0, f64::max);
        if norm(facing) <= 1e-6 * longest * longest {
            continue;
        }
        for i in t {
            let n = wide(mesh.normals[*i as usize]);
            assert!(
                dot(unit(facing), n) > 0.0,
                "{what}: triangle {t:?} faces {:?} but vertex {i} at {:?} has normal {n:?}",
                unit(facing),
                mesh.positions[*i as usize]
            );
        }
    }
}

#[test]
fn every_normal_is_unit_and_on_its_triangles_side() {
    for (what, shape) in shapes() {
        assert_sound(what, &tessellate(&shape, DEFLECTION).unwrap());
    }
}

/// Whether every vertex of the face has the same normal: a flat face.
fn is_flat(mesh: &Mesh, face: &[[u32; 3]]) -> bool {
    let first = mesh.normals[face[0][0] as usize];
    face.iter()
        .flatten()
        .all(|&i| mesh.normals[i as usize] == first)
}

#[test]
fn every_flat_faces_normal_is_its_triangles_facing() {
    // A plane's normal is its triangles' cross product, made unit: on faces made flat
    // (box sides, ends, a revolve's end caps, a chamfer, the floor a pit opens in).
    for (what, shape) in shapes() {
        let mesh = tessellate(&shape, DEFLECTION).unwrap();
        let mut flat = 0;
        for face in faces(&mesh).iter().filter(|f| is_flat(&mesh, f)) {
            flat += 1;
            for t in face {
                let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
                let facing = unit(cross(sub(b, a), sub(c, a)));
                let n = wide(mesh.normals[t[0] as usize]);
                assert!(
                    apart(n, facing) <= 1e-6,
                    "{what}: {t:?} faces {facing:?}, normal {n:?}"
                );
            }
        }
        assert!(
            flat > 0 || ["sphere", "torus", "horn torus"].contains(&what),
            "{what}"
        );
    }
}

/// OCCT's normal at a node where the surface has none (an apex, the point where a torus
/// touches its axis; docs/ports/tessellation.md): the sum of the unit facings of the
/// node's triangles that have an area (their cross product at least 1e-7), made unit. Worked
/// out from the mesh, which keeps each face's own triangles, wound as the face faces.
fn fallback_normal(mesh: &Mesh, vertex: u32) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for t in mesh.triangles.iter().filter(|t| t.contains(&vertex)) {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        let facing = cross(sub(b, a), sub(c, b));
        if norm(facing) >= 1e-7 {
            let u = unit(facing);
            sum = [sum[0] + u[0], sum[1] + u[1], sum[2] + u[2]];
        }
    }
    assert!(
        norm(sum) > 1e-7,
        "vertex {vertex} has no triangle with an area"
    );
    unit(sum)
}

/// The outward normal of a cone about the vertical axis through `center`, radius `radius1`
/// at its base and `radius2` `height` above, at a point `p` off its axis: radial, tilted
/// up by how fast the radius shrinks.
fn cone_normal(p: [f64; 3], radius1: f64, radius2: f64, height: f64, center: [f64; 2]) -> [f64; 3] {
    let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
    let r = (dx * dx + dy * dy).sqrt();
    unit([dx / r, dy / r, (radius1 - radius2) / height])
}

#[test]
fn a_cones_normals_are_the_cones_and_its_apex_its_triangles() {
    // A cone's side has a normal everywhere but its apex: (cos θ, sin θ, (r1 - r2)/h),
    // made unit. At the apex OCCT averages the triangles around each apex node, which still
    // leans out of the solid: up at a top apex, down at a bottom one.
    let triangle = Profile {
        points: vec![[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0, 0.0, 10.0]],
    };
    let z_axis = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };
    let cones = [
        ("pointed at the top", cone(5.0, 0.0, 10.0), 5.0, 0.0),
        ("pointed at the bottom", cone(0.0, 5.0, 10.0), 0.0, 5.0),
        ("frustum", cone(5.0, 2.0, 10.0), 5.0, 2.0),
        (
            "revolved triangle",
            revolve(&triangle, &z_axis, 360.0).unwrap().shape,
            5.0,
            0.0,
        ),
    ];
    for (what, shape, radius1, radius2) in cones {
        let mesh = tessellate(&shape, DEFLECTION).unwrap();
        let mut apexes = 0;
        for (i, (p, n)) in (0u32..).zip(mesh.positions.iter().zip(&mesh.normals)) {
            let n = wide(*n);
            let at = |z: f64| (p[2] - z).abs() <= 1e-9;
            if at(0.0) && n == [0.0, 0.0, -1.0] || at(10.0) && n == [0.0, 0.0, 1.0] {
                continue; // an end
            }
            if (p[0] * p[0] + p[1] * p[1]).sqrt() <= 1e-9 {
                apexes += 1;
                let expected = fallback_normal(&mesh, i);
                assert!(
                    apart(n, expected) <= 1e-6,
                    "{what}: apex {n:?}, expected {expected:?}"
                );
                let up = if radius2 == 0.0 { 1.0 } else { -1.0 };
                assert!(
                    n[2] * up > 0.0,
                    "{what}: the apex's normal {n:?} leans into the cone"
                );
                continue;
            }
            let expected = cone_normal(*p, radius1, radius2, 10.0, [0.0, 0.0]);
            assert!(
                apart(n, expected) <= 1e-6,
                "{what}: {n:?} at {p:?}, expected {expected:?}"
            );
        }
        assert_eq!(apexes > 0, radius1 == 0.0 || radius2 == 0.0, "{what}");
    }
}

#[test]
fn a_pits_normals_are_the_cones_reversed() {
    // The pit's face is the cone's surface reversed: its normals point into the pit, toward
    // the axis and down, the apex's (averaged from its triangles) down too. Apex at
    // (10, 10, 7); the cone narrows from 5 to 0 over 8.
    let mesh = tessellate(&box_with_a_conical_pit(), DEFLECTION).unwrap();
    let (mut on_pit, mut apexes) = (0, 0);
    for (i, (p, n)) in (0u32..).zip(mesh.positions.iter().zip(&mesh.normals)) {
        let n = wide(*n);
        let (dx, dy) = (p[0] - 10.0, p[1] - 10.0);
        let off_axis = (dx * dx + dy * dy).sqrt();
        // On the cone: its radius at height z is 5 (7 - z) / 8.
        if (off_axis - 5.0 * (7.0 - p[2]) / 8.0).abs() > 1e-9 || p[2] < 0.0 {
            continue;
        }
        if n[2] == -1.0 || n[2] == 1.0 {
            continue; // the box's floor, where the pit opens
        }
        on_pit += 1;
        if off_axis <= 1e-9 {
            apexes += 1;
            let expected = fallback_normal(&mesh, i);
            assert!(
                apart(n, expected) <= 1e-6,
                "apex {n:?}, expected {expected:?}"
            );
            assert!(n[2] < 0.0, "the apex's normal {n:?} leans into the solid");
        } else {
            let outward = cone_normal(*p, 5.0, 0.0, 8.0, [10.0, 10.0]);
            assert!(apart(n, outward.map(|c| -c)) <= 1e-6, "{n:?} at {p:?}");
        }
    }
    assert!(
        on_pit > 0 && apexes > 0,
        "{on_pit} on the pit, {apexes} at its apex"
    );
}

/// The outward normal of a torus about the z axis at a point `p` on it: from the center of
/// the tube's section through `p`.
fn torus_normal(p: [f64; 3], major: f64, minor: f64) -> [f64; 3] {
    let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
    let center = [major * p[0] / r, major * p[1] / r, 0.0];
    sub(p, center).map(|c| c / minor)
}

#[test]
fn a_torus_normals_point_from_its_tube() {
    let mesh = tessellate(&torus(10.0, 2.0), DEFLECTION).unwrap();
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let expected = torus_normal(*p, 10.0, 2.0);
        assert!(apart(wide(*n), expected) <= 1e-6, "{n:?} at {p:?}");
    }
}

#[test]
fn a_horn_torus_normal_where_it_touches_its_axis_is_its_triangles() {
    // A tube of radius 5 whose center circle has radius 5 touches the axis at the origin,
    // where the surface has no normal: there OCCT averages the triangles around the node.
    // Everywhere else, the tube's normal.
    let mesh = tessellate(&torus(5.0, 5.0), DEFLECTION).unwrap();
    let mut at_center = 0;
    for (i, (p, n)) in (0u32..).zip(mesh.positions.iter().zip(&mesh.normals)) {
        let n = wide(*n);
        if norm(*p) <= 1e-9 {
            at_center += 1;
            let expected = fallback_normal(&mesh, i);
            assert!(
                apart(n, expected) <= 1e-6,
                "{n:?} at the center, expected {expected:?}"
            );
            continue;
        }
        let expected = torus_normal(*p, 5.0, 5.0);
        assert!(apart(n, expected) <= 1e-6, "{n:?} at {p:?}");
    }
    assert!(at_center > 0);
}

#[test]
fn a_cavitys_normals_point_to_its_center_even_at_its_poles() {
    // The cavity's face is a sphere's surface reversed: every normal points from the vertex
    // to the center (10, 10, 10), the poles included.
    let mesh = tessellate(&box_with_a_cavity(), DEFLECTION).unwrap();
    let mut on_cavity = 0;
    let mut poles = 0;
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let from_center = sub(*p, [10.0; 3]);
        if (norm(from_center) - 5.0).abs() > 1e-9 {
            continue;
        }
        on_cavity += 1;
        if from_center[0] == 0.0 && from_center[1] == 0.0 {
            poles += 1;
        }
        let inward = unit(from_center).map(|c| -c);
        assert!(apart(wide(*n), inward) <= 1e-6, "{n:?} at {p:?}");
    }
    assert!(
        on_cavity > 0 && poles > 0,
        "{on_cavity} on the cavity, {poles} at poles"
    );
}

#[test]
fn a_filleted_corners_normals_point_from_its_ball_and_axes() {
    // Three edges of radius 2 meeting at the origin's corner: the edges become quarter
    // cylinders about lines 2 in from two faces, and the corner an eighth of a ball of
    // radius 2 about (2, 2, 2). Every vertex of a curved face has the normal from the axis
    // or center it's rounded about.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let rounded = fillet(&b, &edges_at(&b, [0.0; 3]), 2.0).unwrap().shape;
    let mesh = tessellate(&rounded, 0.01).unwrap();
    let (mut curved, mut on_ball) = (0, 0);
    for face in faces(&mesh).iter().filter(|f| !is_flat(&mesh, f)) {
        curved += 1;
        for &i in face.iter().flatten() {
            let (p, n) = (mesh.positions[i as usize], wide(mesh.normals[i as usize]));
            let near = |k: usize| p[k] <= 2.0 + 1e-9;
            let center = match (near(0), near(1), near(2)) {
                (true, true, true) => [2.0, 2.0, 2.0],
                (true, true, false) => [2.0, 2.0, p[2]],
                (true, false, true) => [2.0, p[1], 2.0],
                (false, true, true) => [p[0], 2.0, 2.0],
                _ => panic!("{p:?} is on no blend"),
            };
            if near(0) && near(1) && near(2) {
                on_ball += 1;
            }
            let out = sub(p, center);
            assert!(
                (norm(out) - 2.0).abs() <= 1e-4,
                "{p:?} is {} from {center:?}",
                norm(out)
            );
            assert!(
                apart(n, unit(out)) <= 1e-4,
                "{n:?} at {p:?}, expected {:?}",
                unit(out)
            );
        }
    }
    assert_eq!(curved, 4);
    assert!(on_ball > 0);
}

#[test]
fn a_moved_shapes_normals_are_its_own() {
    // Moving a shape moves its vertices and nothing else: each vertex keeps its normal (to
    // f32's precision), however far the move. OCCT may mesh a moved face differently (a
    // cone moved 0.1 gets other triangles, a horn torus moved 3.7e5 other vertices); that
    // mesh's normals must still be sound.
    for (what, shape) in shapes() {
        let here = tessellate(&shape, DEFLECTION).unwrap();
        for offset in [[0.1, 0.0, 0.0], [1e5, -2e5, 3e5]] {
            let there = tessellate(&moved(&shape, offset), DEFLECTION).unwrap();
            assert_sound(what, &there);
            if there.triangles != here.triangles {
                continue;
            }
            for (a, b) in here.normals.iter().zip(&there.normals) {
                assert!(
                    apart(wide(*a), wide(*b)) <= 1e-6,
                    "{what} by {offset:?}: {a:?} became {b:?}"
                );
            }
        }
    }
}

/// The same mesh, but vertices may move by a rounding error: a B-rep read back can give a
/// few vertices that differ in their last bits (a revolved cone's do, by 2e-15).
fn assert_same_mesh(what: &str, a: &Mesh, b: &Mesh) {
    assert_eq!(a.normals, b.normals, "{what}");
    assert_eq!(a.triangles, b.triangles, "{what}");
    assert_eq!(a.face_triangles, b.face_triangles, "{what}");
    assert_eq!(a.edge_point_counts, b.edge_point_counts, "{what}");
    assert_eq!(a.positions.len(), b.positions.len(), "{what}");
    for (p, q) in a.positions.iter().zip(&b.positions) {
        assert!(apart(*p, *q) <= 1e-12, "{what}: {p:?} and {q:?}");
    }
}

#[test]
fn a_shape_read_back_from_its_brep_has_the_same_normals() {
    // Stored and read back: bit for bit the same normals.
    for (what, shape) in shapes() {
        let back = from_brep(&to_brep(&shape).unwrap()).unwrap();
        let (a, b) = (
            tessellate(&shape, DEFLECTION).unwrap(),
            tessellate(&back, DEFLECTION).unwrap(),
        );
        assert_same_mesh(what, &a, &b);
        // And read back again, exactly the same: the round trip settles at once.
        let again = from_brep(&to_brep(&back).unwrap()).unwrap();
        assert_eq!(tessellate(&again, DEFLECTION).unwrap(), b, "{what}");
    }
}

#[test]
fn a_workers_mesh_is_the_bridges_bit_for_bit() {
    // Across the process boundary, a worker meshes the shape it reads from the B-rep:
    // postcard keeps every f32 normal exactly.
    let mut worker = kernel();
    for (what, shape) in shapes() {
        let brep = to_brep(&shape).unwrap();
        let read_back = from_brep(&brep).unwrap();
        let data = ShapeData::unchecked(Blob::inline(brep));
        let remote = worker.tessellate(&data, DEFLECTION).unwrap();
        assert_eq!(
            remote,
            tessellate(&read_back, DEFLECTION).unwrap(),
            "{what}"
        );
    }
}

#[test]
fn a_thin_slabs_normals_are_its_axes() {
    // 100 × 100 × 1e-6: the two big faces 1e-6 apart still point opposite ways, and the
    // edges' faces sideways.
    let slab = make_box(100.0, 100.0, 1e-6).unwrap();
    let mesh = tessellate(&slab, 0.05).unwrap();
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let axis = (0..3).find(|&k| n[k] != 0.0).unwrap();
        let size = [100.0, 100.0, 1e-6];
        let mut expected = [0.0f32; 3];
        expected[axis] = if p[axis] == 0.0 { -1.0 } else { 1.0 };
        assert_eq!(*n, expected, "at {p:?}");
        assert!(p[axis] == 0.0 || p[axis] == size[axis], "{p:?}");
    }
}

#[test]
fn the_largest_spheres_normals_point_from_its_center() {
    // At MAX_LENGTH: |dP/du × dP/dv| is r² = 1e12, far from overflowing.
    let mesh = tessellate(&sphere(1e6), 1e3).unwrap();
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        assert!(apart(wide(*n), unit(*p)) <= 1e-6, "{n:?} at {p:?}");
    }
}

/// The worst distance between a normal of a ball of `radius` about the origin and the
/// direction from the center to its vertex, and where, meshed as finely as the ball's size
/// allows (a hundredth of the radius).
fn worst_ball_normal(radius: f64) -> (f64, [f64; 3]) {
    let mesh = tessellate(&sphere(radius), radius / 100.0).unwrap();
    mesh.positions
        .iter()
        .zip(&mesh.normals)
        .map(|(p, n)| (apart(wide(*n), unit(*p)), *p))
        .fold(
            (0.0, [0.0; 3]),
            |worst, next| if next.0 > worst.0 { next } else { worst },
        )
}

#[test]
fn a_small_balls_north_pole_normal_points_into_it_as_occt_gives_it() {
    // Accepted as FreeCAD's display has it (docs/ports/tessellation.md, What OCCT gets
    // wrong): a ball of radius under about 0.0316 mm has the normal at its north pole
    // pointing straight into it, (0, 0, -1). OCCT's GeomLib::NormEstim takes a pole's
    // normal from second derivatives, with either sign, and orients it by the surface's
    // normal a step (1e-5 in v) away, but skips that when |dP/du × dP/dv| there, r² · 1e-5,
    // is under its absolute 1e-8. Every other normal is radial. Pinned, so a change in OCCT
    // shows.
    for radius in [0.03, 0.01, 1e-3] {
        let mesh = tessellate(&sphere(radius), radius / 100.0).unwrap();
        let mut north = 0;
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            let at_north = norm([p[0], p[1], 0.0]) <= radius * 1e-9 && p[2] > 0.0;
            let expected = if at_north { [0.0, 0.0, -1.0] } else { unit(*p) };
            north += usize::from(at_north);
            assert!(
                apart(wide(*n), expected) <= 1e-6,
                "radius {radius}: {n:?} at {p:?}, pinned {expected:?}"
            );
        }
        assert!(north > 0, "radius {radius}: no vertex at the north pole");
    }
}

#[test]
fn a_small_hollows_north_pole_normal_points_into_the_solid_as_occt_gives_it() {
    // The same, reversed: a hollow of radius 0.01 in a 0.1 box has at its north pole the
    // normal (0, 0, 1), pointing into the material, where out of the solid is down, to the
    // hollow's center. Every other normal on the hollow points to its center. Pinned.
    let center = [0.05; 3];
    let hollow = cut(
        &make_box(0.1, 0.1, 0.1).unwrap(),
        &moved(&sphere(0.01), center),
    );
    let mesh = tessellate(&hollow, 1e-4).unwrap();
    let mut north = 0;
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let from_center = sub(*p, center);
        if (norm(from_center) - 0.01).abs() > 1e-12 {
            continue;
        }
        let at_north = norm([from_center[0], from_center[1], 0.0]) <= 1e-11 && from_center[2] > 0.0;
        let expected = if at_north {
            [0.0, 0.0, 1.0]
        } else {
            unit(from_center).map(|c| -c)
        };
        north += usize::from(at_north);
        assert!(
            apart(wide(*n), expected) <= 1e-6,
            "{n:?} at {p:?}, pinned {expected:?}"
        );
    }
    assert!(north > 0, "no vertex at the hollow's north pole");
}

#[test]
fn a_ball_just_over_the_size_where_poles_flip_points_out() {
    // Just over r = 0.0316 the pole's orientation step works: every normal is radial.
    for radius in [0.035, 0.05, 1.0] {
        let (worst, at) = worst_ball_normal(radius);
        assert!(
            worst <= 1e-6,
            "radius {radius}: the normal at {at:?} is {worst} off"
        );
    }
}

/// Sizes biased to the ends of what the kernel takes (MAX_LENGTH is 1e6; under 0.0316 a
/// ball's pole flips, which `a_small_balls_north_pole_normal_points_into_it_as_occt_gives_it`
/// covers) and to round
/// and unround values between.
fn size() -> impl Strategy<Value = f64> {
    prop_oneof![
        1 => Just(0.05),
        1 => Just(1.0),
        1 => Just(7.3),
        1 => Just(1e3),
        1 => Just(1e6),
        3 => 0.05f64..20.0,
        2 => 0.05f64..1e6,
    ]
}

/// 10 to the power `e`, by multiplying (no platform math).
fn decade(e: i32) -> f64 {
    (0..e.abs()).fold(1.0, |r, _| if e > 0 { r * 10.0 } else { r / 10.0 })
}

/// A second size, from the first: equal, up to a thousand times either way (a slender cone
/// past that is `a_needles_apex_normal_is_one_rulings_as_occt_gives_it`'s), or zero where allowed, kept
/// within what the kernel takes.
fn other(first: f64, zero: bool) -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(if zero { 0.0 } else { 1.0 }),
        Just(1e-3),
        Just(0.5),
        Just(1.0),
        Just(2.0),
        Just(1e3),
        (-3i32..3, 1.0f64..10.0).prop_map(|(e, m)| m * decade(e)),
    ]
    .prop_map(move |ratio| {
        if ratio == 0.0 {
            0.0
        } else {
            (first * ratio).clamp(0.05, 1e6)
        }
    })
}

/// A primitive of every kind but the box (all flat faces, which the targeted tests cover).
fn primitive() -> impl Strategy<Value = Primitive> {
    prop_oneof![
        size().prop_map(|radius| Primitive::Sphere { radius }),
        size()
            .prop_flat_map(|radius| (Just(radius), other(radius, false)))
            .prop_map(|(radius, height)| Primitive::Cylinder { radius, height }),
        size()
            .prop_flat_map(|r| (Just(r), other(r, true), other(r, false), any::<bool>()))
            .prop_map(|(r, s, height, up)| {
                let (radius1, radius2) = if up { (r, s) } else { (s, r) };
                // No more slender than 1 in 500: a needle's apex is
                // `a_needles_apex_normal_is_one_rulings_as_occt_gives_it`'s.
                let height = height.min(r.max(s) * 500.0);
                Primitive::Cone {
                    radius1,
                    radius2,
                    height,
                }
            }),
        // Not a horn torus (the tube touching the axis), whose center has no normal: OCCT's
        // fallback there is accepted as it is (docs/ports/tessellation.md).
        size()
            .prop_flat_map(|major| (Just(major), other(major, false)))
            .prop_map(|(major_radius, minor)| Primitive::Torus {
                major_radius,
                minor_radius: minor.min(major_radius * 0.999),
            }),
    ]
}

/// The surface's own outward normal at `p`, a vertex with normal `n` (`tol` from the
/// surface), from the primitive's closed form; None where it has none (an apex, a horn
/// torus's center). A vertex on a rim, where two faces meet, may have either face's.
fn expected_normal(primitive: &Primitive, p: [f64; 3], n: [f64; 3], tol: f64) -> Option<[f64; 3]> {
    let off_axis = (p[0] * p[0] + p[1] * p[1]).sqrt();
    let closest = |candidates: Vec<[f64; 3]>| {
        assert!(!candidates.is_empty(), "{primitive:?}: {p:?} is on no face");
        candidates
            .into_iter()
            .min_by(|a, b| apart(n, *a).total_cmp(&apart(n, *b)))
    };
    match *primitive {
        Primitive::Sphere { .. } => Some(unit(p)),
        Primitive::Cylinder { radius, height } => {
            let mut faces = Vec::new();
            if p[2].abs() <= tol {
                faces.push([0.0, 0.0, -1.0]);
            }
            if (p[2] - height).abs() <= tol {
                faces.push([0.0, 0.0, 1.0]);
            }
            if (off_axis - radius).abs() <= tol {
                faces.push([p[0] / off_axis, p[1] / off_axis, 0.0]);
            }
            closest(faces)
        }
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => {
            let slope = (radius1 - radius2) / height;
            let radius_here = radius1 - slope * p[2];
            if off_axis <= tol && radius_here <= tol * (1.0 + slope.abs()) {
                return None; // the apex
            }
            let mut faces = Vec::new();
            if p[2].abs() <= tol && radius1 > 0.0 {
                faces.push([0.0, 0.0, -1.0]);
            }
            if (p[2] - height).abs() <= tol && radius2 > 0.0 {
                faces.push([0.0, 0.0, 1.0]);
            }
            if (off_axis - radius_here).abs() <= tol * (1.0 + slope.abs()) {
                faces.push(cone_normal(p, radius1, radius2, height, [0.0, 0.0]));
            }
            closest(faces)
        }
        Primitive::Torus {
            major_radius,
            minor_radius,
        } => {
            if off_axis <= tol {
                return None; // a horn torus's center
            }
            Some(torus_normal(p, major_radius, minor_radius))
        }
        Primitive::Box { .. } => None,
    }
}

/// The primitive's largest dimension.
fn extent(primitive: &Primitive) -> f64 {
    match *primitive {
        Primitive::Sphere { radius } => radius,
        Primitive::Cylinder { radius, height } => radius.max(height),
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => radius1.max(radius2).max(height),
        Primitive::Torus { major_radius, .. } => major_radius,
        Primitive::Box { dx, dy, dz } => dx.max(dy).max(dz),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn every_primitives_normals_are_its_surfaces(primitive in primitive()) {
        let Ok(shape) = make(&primitive) else {
            // Out of range (a radius over MAX_LENGTH, a torus whose tube crosses its axis):
            // refused, which other tests cover.
            return Ok(());
        };
        let size = extent(&primitive);
        let mesh = tessellate(&shape, size / 50.0).unwrap();
        assert_sound(&format!("{primitive:?}"), &mesh);
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            let n = wide(*n);
            if let Some(expected) = expected_normal(&primitive, *p, n, 1e-9 * size.max(1.0)) {
                prop_assert!(
                    apart(n, expected) <= 1e-6,
                    "{:?}: {:?} at {:?}, expected {:?}", primitive, n, p, expected
                );
            }
        }
    }
}

#[test]
fn a_needles_apex_normal_is_one_rulings_as_occt_gives_it() {
    // Accepted as FreeCAD's display has it (docs/ports/tessellation.md, What OCCT gets
    // wrong): a cone 1000 times as tall as its base radius (1 in 1000, or more slender)
    // has at its apex the normal of one ruling, not the average of the apex's triangles
    // that a stouter cone gets. At a top apex it's the ruling's reversed, pointing into the
    // cone; at a bottom apex the ruling's own. Same cause as the small ball: GeomLib::
    // NormEstim compares |dP/du| ~ 1e-5 r/h a step from the apex with an absolute 1e-8.
    // Pinned, so a change in OCCT shows.
    let lean = 1e-3 / (1.0f64 + 1e-6).sqrt();
    let across = 1.0 / (1.0f64 + 1e-6).sqrt();
    for (radius1, radius2, pinned) in [
        (1.0, 0.0, [-across, 0.0, -lean]),
        (0.0, 1.0, [across, 0.0, -lean]),
    ] {
        let mesh = tessellate(&cone(radius1, radius2, 1000.0), 10.0).unwrap();
        let mut apexes = 0;
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            if norm([p[0], p[1], 0.0]) > 1e-9 {
                continue;
            }
            apexes += 1;
            assert!(
                apart(wide(*n), pinned) <= 1e-6,
                "{radius1} to {radius2}: apex normal {n:?}, pinned {pinned:?}"
            );
        }
        assert!(apexes > 0);
    }
}

// ----------------------------------------------------------------------------------------
// Locations no Silico operation makes, read from B-reps.

/// Every double's bytes, in both byte orders, so the search finds them whichever BinTools
/// wrote.
fn bytes_of(values: &[f64], little: bool) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        })
        .collect()
}

/// `shape`, moved by an unusual offset, with that move's location in its B-rep (12 doubles:
/// three rows of a 3 × 3 matrix, each followed by its translation) rewritten as `rows`.
fn relocated(shape: &Shape, rows: [[f64; 4]; 3]) -> Shape {
    from_brep(&relocated_brep(shape, rows)).unwrap()
}

/// The B-rep of [`relocated`]'s shape, its checksum fixed.
fn relocated_brep(shape: &Shape, rows: [[f64; 4]; 3]) -> Vec<u8> {
    let offset = [101.25, 202.5, 303.75];
    let brep = to_brep(&moved(shape, offset)).unwrap();
    let wanted: Vec<f64> = rows.iter().flatten().copied().collect();
    patched_brep(&brep, &location(IDENTITY, offset), &wanted)
}

/// A quarter turn about z, then a move to (100, 0, 0): (x, y, z) to (100 - y, x, z).
const QUARTER_TURN: [[f64; 4]; 3] = [
    [0.0, -1.0, 0.0, 100.0],
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// A turn about x by the angle whose cosine is 0.6 and sine 0.8 (exact).
const TILT: [[f64; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 0.6, -0.8, 0.0],
    [0.0, 0.8, 0.6, 0.0],
];

/// A mirror in the plane x = 0.
const MIRROR: [[f64; 4]; 3] = [
    [-1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// Twice the size.
const DOUBLE: [[f64; 4]; 3] = [
    [2.0, 0.0, 0.0, 0.0],
    [0.0, 2.0, 0.0, 0.0],
    [0.0, 0.0, 2.0, 0.0],
];

fn apply(rows: [[f64; 4]; 3], v: [f64; 3]) -> [f64; 3] {
    rows.map(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2])
}

/// Each flat-faced triangle's facing, which every one of its vertex normals must equal.
fn assert_flat_normals_match_triangles(what: &str, mesh: &Mesh) {
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        let facing = unit(cross(sub(b, a), sub(c, a)));
        for i in t {
            let n = wide(mesh.normals[*i as usize]);
            assert!(
                apart(n, facing) <= 1e-6,
                "{what}: triangle {t:?} faces {facing:?}, vertex {i} has {n:?}"
            );
        }
    }
}

#[test]
fn a_turned_boxs_normals_turn_with_it() {
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let plain = tessellate(&b, 0.1).unwrap();
    assert_flat_normals_match_triangles("box", &plain);
    for (what, rows) in [("quarter turn", QUARTER_TURN), ("tilt", TILT)] {
        let mesh = tessellate(&relocated(&b, rows), 0.1).unwrap();
        assert_flat_normals_match_triangles(what, &mesh);
        // And each is the box's own normal at that vertex, turned.
        assert_eq!(mesh.normals.len(), plain.normals.len());
        for (n, original) in mesh.normals.iter().zip(&plain.normals) {
            let expected = apply(rows, wide(*original));
            assert!(
                apart(wide(*n), expected) <= 1e-6,
                "{what}: {n:?}, expected {expected:?}"
            );
        }
    }
}

#[test]
fn a_turned_cylinders_normals_are_radial_about_its_turned_axis() {
    // Tilted about x, the cylinder's axis runs along (0, -0.8, 0.6) through the origin.
    let mesh = tessellate(&relocated(&cylinder(5.0, 10.0), TILT), DEFLECTION).unwrap();
    let axis = [0.0, -0.8, 0.6];
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let n = wide(*n);
        let along = dot(*p, axis);
        let radial = sub(*p, axis.map(|c| c * along));
        if (norm(radial) - 5.0).abs() <= 1e-9 && dot(n, axis).abs() < 0.5 {
            assert!(apart(n, unit(radial)) <= 1e-6, "{n:?} at {p:?}");
        } else {
            let end = if along < 5.0 { -1.0 } else { 1.0 };
            assert!(apart(n, axis.map(|c| c * end)) <= 1e-6, "{n:?} at {p:?}");
        }
    }
}

#[test]
fn a_brep_placed_by_a_mirror_or_a_scale_is_refused() {
    // OCCT sets a location that mirrors or scales unless asked to check
    // (TopoDS_Shape::validateTransformation), and reads one from bytes, and then its
    // operations disagree about which side of the shape is solid: a mirrored box measures
    // volume -6000, a big box less it is the box, and its mesh drew it as solid. So
    // from_brep refuses it, down to a scale just past OCCT's ScalePrec (1e-14), and reads
    // one just within it. (Found by /break-it and the review.)
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let scale = |s: f64| [[s, 0.0, 0.0, 0.0], [0.0, s, 0.0, 0.0], [0.0, 0.0, s, 0.0]];
    for (what, rows) in [
        ("mirrored", MIRROR),
        ("doubled", DOUBLE),
        ("scaled by 1 + 3e-14", scale(1.0 + 3e-14)),
    ] {
        assert_refused(&relocated_brep(&b, rows), what, "mirrors or scales");
    }
    from_brep(&relocated_brep(&b, scale(1.0 + 3e-15))).unwrap();
}

#[test]
fn a_mirror_on_a_second_placement_of_a_shared_shape_is_refused() {
    // A box fused with a moved copy of itself: the copy's faces share the box's, placed
    // again. Each placement is checked, not each shared shape once.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let o = [101.25, 202.5, 303.75];
    let pair = boolean(BooleanOp::Fuse, &b, &moved(&b, o)).unwrap().shape;
    let bad = patched_brep(
        &to_brep(&pair).unwrap(),
        &location(IDENTITY, o),
        &location(MIRROR, o),
    );
    assert_refused(&bad, "a mirrored second placement", "mirrors or scales");
}

/// A B-rep OCCT wrote (tests/fixtures/brep), with Silico's checksum.
fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/brep")
        .join(name);
    with_crc(std::fs::read(path).unwrap())
}

#[test]
fn a_mirror_raised_to_an_even_power_is_read() {
    // powered_far.bin's link rewritten: a mirror (moving 5 along x) raised to 2 is the
    // identity, so the box is read where it was made, and is sound.
    let mirrored = patched_brep(
        &fixture("powered_far.bin"),
        &location(IDENTITY, [1e300, 0.0, 0.0]),
        &location(MIRROR, [5.0, 0.0, 0.0]),
    );
    let mut body = mirrored[..mirrored.len() - 4].to_vec();
    // The fixture's power (its README).
    let power = 1_000_000_000i32.to_le_bytes();
    let at: Vec<usize> = body
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == power)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "the power appears {} times", at.len());
    body[at[0]..at[0] + 4].copy_from_slice(&2i32.to_le_bytes());
    let shape = from_brep(&with_crc(body)).unwrap();
    let summary = summarize(&shape).unwrap();
    assert!(
        (summary.volume - 6000.0).abs() <= 1e-9 && summary.valid,
        "{summary:?}"
    );
}

#[test]
fn a_location_raised_to_a_power_is_checked_as_raised() {
    // A link of a location is a datum raised to a power: a finite move raised to 1e9 is a
    // move past what a double holds, and a scale within OCCT's ScalePrec raised to 1e9 is a
    // scale of 1.000005. Each is refused (the link's datum alone would pass).
    assert_refused(
        &fixture("powered_far.bin"),
        "a move raised far",
        "aren't finite",
    );
    assert_refused(
        &fixture("powered_scale.bin"),
        "a scale raised past ScalePrec",
        "mirrors or scales",
    );
}

#[test]
fn internal_and_external_faces_mesh_with_triangles_and_normals_agreeing() {
    // A box whose face at x = 0 is INTERNAL, or EXTERNAL, in its shell (made in the FreeCAD
    // oracle: tests/fixtures/brep/README.md). Silico winds a face's triangles the other way
    // only when it's REVERSED, as it reverses its normals then, so on these faces both stay
    // as the plane has them, +x (the oracle's normal there), and agree. FreeCAD swaps the
    // corners of every face that isn't FORWARD but reverses normals only on REVERSED ones,
    // so there its triangles face -x against normals of +x.
    for name in ["box_internal_face1.bin", "box_external_face1.bin"] {
        let mesh = tessellate(&from_brep(&fixture(name)).unwrap(), 0.1).unwrap();
        assert_flat_normals_match_triangles(name, &mesh);
        let on_face: Vec<_> = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .filter(|(p, n)| p[0] == 0.0 && n[0] != 0.0)
            .collect();
        assert!(
            !on_face.is_empty(),
            "{name}: no vertex on the face at x = 0"
        );
        for (p, n) in on_face {
            assert_eq!(*n, [1.0, 0.0, 0.0], "{name}: at {p:?}");
        }
    }
}

#[test]
fn a_bare_face_with_no_surface_is_refused() {
    // No surface, no edges, and no triangles: meshonly.bin with its face's triangulation
    // reference taken out (the flag at 0x135 from 2 to 0, and the 4-byte index after it).
    let raw = &fixture("meshonly.bin");
    let raw = &raw[..raw.len() - 4];
    assert_eq!((raw[0x123], raw[0x135]), (4, 2), "the face record moved");
    let mut body = raw[..0x135].to_vec();
    body.push(0);
    body.extend_from_slice(&raw[0x13a..]);
    assert_refused(
        &with_crc(body),
        "a face with no surface, edges, or triangles",
        "no surface",
    );
}

#[test]
fn a_face_of_only_a_triangulation_is_refused() {
    // No surface and no edges, only triangles: what OCCT's STEP and glTF readers make of a
    // tessellated face. OCCT measures and meshes it, but a boolean on it segfaults (as in
    // FreeCAD), so it's refused until Silico can work on meshes; through a worker, a
    // boolean with it is refused, not a crash.
    let bytes = fixture("meshonly.bin");
    assert_refused(&bytes, "a face of only a triangulation", "no surface");
    let err = from_brep(&bytes).err().unwrap();
    assert!(
        err.suggestion().is_some_and(|s| s.contains("mesh")),
        "{err:?}"
    );
    let mut worker = kernel();
    let face = ShapeData::unchecked(Blob::inline(bytes));
    let block = worker
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    for op in [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common] {
        for (base, tool) in [(&block, &face), (&face, &block)] {
            let err = worker.boolean(op, base, tool).unwrap_err();
            assert_eq!(
                err.code(),
                silico_core::ErrorCode::InvalidInput,
                "{op:?}: {err:#?}"
            );
        }
    }
}

/// `body` with Silico's checksum after it.
fn with_crc(mut body: Vec<u8>) -> Vec<u8> {
    body.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
    body
}

/// `brep` with the doubles `from` rewritten as `to` (found once, in either byte order),
/// its checksum fixed.
fn patched_brep(brep: &[u8], from: &[f64], to: &[f64]) -> Vec<u8> {
    let body = &brep[..brep.len() - 4];
    let mut patched = None;
    for little in [true, false] {
        let f = bytes_of(from, little);
        let at: Vec<usize> = body
            .windows(f.len())
            .enumerate()
            .filter(|(_, w)| *w == f.as_slice())
            .map(|(i, _)| i)
            .collect();
        assert!(at.len() <= 1, "found {} times", at.len());
        if let Some(&i) = at.first() {
            let mut b = body.to_vec();
            b[i..i + f.len()].copy_from_slice(&bytes_of(to, little));
            patched = Some(b);
        }
    }
    with_crc(patched.expect("the values aren't in the B-rep"))
}

/// The 12 doubles of a location with `rows`' 3 × 3 matrix and the translation `o`
/// (`rows`' own translation column is ignored).
fn location(rows: [[f64; 4]; 3], o: [f64; 3]) -> Vec<f64> {
    rows.iter()
        .zip(o)
        .flat_map(|(r, t)| [r[0], r[1], r[2], t])
        .collect()
}

const IDENTITY: [[f64; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// `brep` is refused as invalid input, with a suggestion, and a message saying `why`.
fn assert_refused(brep: &[u8], what: &str, why: &str) {
    let err = from_brep(brep)
        .err()
        .unwrap_or_else(|| panic!("{what} was accepted"));
    assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{what}: {err}");
    assert!(err.suggestion().is_some(), "{what}: {err}");
    assert!(err.message().contains(why), "{what}: {err}");
}

#[test]
fn a_mirror_or_scale_on_a_face_inside_the_shape_is_refused() {
    // A cut keeps the ball's face with the ball's location; the root has none. Mirrored,
    // the cavity measured 8000 + 523.6 and its triangles wound against their normals.
    let o = [10.25, 10.5, 10.75];
    let ball = translate(&sphere(5.0), o).unwrap().shape;
    let cavity = cut(&make_box(20.0, 20.0, 20.0).unwrap(), &ball);
    let brep = to_brep(&cavity).unwrap();
    for (what, rows) in [("mirrored", MIRROR), ("doubled", DOUBLE)] {
        let bad = patched_brep(&brep, &location(IDENTITY, o), &location(rows, o));
        assert_refused(&bad, &format!("a {what} face inside"), "mirrors or scales");
    }
}

#[test]
fn a_mirror_later_in_a_locations_chain_is_refused() {
    // Balls kept unsplit by a second fuse are placed by the chain [o1, o2]: each link is
    // checked, not only the first.
    let (o1, o2) = ([10.25, 10.5, 10.75], [100.125, 100.375, 100.625]);
    let a = translate(&sphere(1.0), o1).unwrap().shape;
    let b = translate(&sphere(1.0), [-40.25, -40.5, -40.75])
        .unwrap()
        .shape;
    let pair = translate(&boolean(BooleanOp::Fuse, &a, &b).unwrap().shape, o2)
        .unwrap()
        .shape;
    let c = translate(&sphere(1.0), [300.25, 300.5, 300.75])
        .unwrap()
        .shape;
    let brep = to_brep(&boolean(BooleanOp::Fuse, &pair, &c).unwrap().shape).unwrap();
    // Every link mirrored: each chain is mirror · mirror, a rotation as a whole, which OCCT
    // would accept; each link is checked, so it's refused.
    let mut every = brep.clone();
    for o in [o1, [-40.25, -40.5, -40.75], o2] {
        every = patched_brep(&every, &location(IDENTITY, o), &location(MIRROR, o));
    }
    assert_refused(&every, "two mirrors in each chain", "mirrors or scales");
    for o in [o1, o2] {
        let bad = patched_brep(&brep, &location(IDENTITY, o), &location(MIRROR, o));
        assert_refused(
            &bad,
            &format!("a mirror in the link moved by {o:?}"),
            "mirrors or scales",
        );
    }
}

#[test]
fn a_brep_placed_by_a_location_that_isnt_finite_is_refused() {
    // The move's location with a translation, or a matrix entry, that isn't a number or
    // isn't finite: summarize gave a plausible area beside a center of NaN.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let mut nan_matrix = IDENTITY;
    nan_matrix[1][1] = f64::NAN;
    for (what, rows, o) in [
        ("a NaN translation", IDENTITY, [f64::NAN; 3]),
        (
            "an infinite translation",
            IDENTITY,
            [f64::INFINITY, 0.0, 0.0],
        ),
        ("a NaN in the matrix", nan_matrix, [0.0; 3]),
        ("a NaN y translation", IDENTITY, [0.0, f64::NAN, 0.0]),
        (
            "an infinite z translation",
            IDENTITY,
            [0.0, 0.0, f64::INFINITY],
        ),
    ] {
        let offset = [101.25, 202.5, 303.75];
        let brep = to_brep(&moved(&b, offset)).unwrap();
        let bad = patched_brep(&brep, &location(IDENTITY, offset), &location(rows, o));
        assert_refused(&bad, what, "aren't finite");
    }
}

/// A box's B-rep with its first face's surface index set to 0. A face record
/// (BinTools_ShapeSet::WriteShape): type byte 4, a bool, its tolerance (f64), its surface
/// (i32), the surface's location (i32), and a byte 0 for no triangulation.
fn surfaceless_face_brep() -> Vec<u8> {
    let brep = to_brep(&make_box(10.0, 20.0, 30.0).unwrap()).unwrap();
    let body = &brep[..brep.len() - 4];
    let shapes = body.windows(7).position(|w| w == b"TShapes").unwrap();
    let mut body = body.to_vec();
    let mut i = shapes;
    loop {
        let tolerance = f64::from_le_bytes(body[i + 2..i + 10].try_into().unwrap());
        let surface = i32::from_le_bytes(body[i + 10..i + 14].try_into().unwrap());
        let located = i32::from_le_bytes(body[i + 14..i + 18].try_into().unwrap());
        if body[i] == 4
            && body[i + 1] <= 1
            && (1e-9..1e-3).contains(&tolerance)
            && (1..=6).contains(&surface)
            && located == 0
            && body[i + 18] == 0
        {
            body[i + 10..i + 14].copy_from_slice(&0i32.to_le_bytes());
            break;
        }
        i += 1;
    }
    with_crc(body)
}

#[test]
fn a_face_with_no_surface_is_refused_not_crashed() {
    // OCCT reads it, and its validity check (run by summarize and translate) segfaulted on
    // it, crashing every worker it was given to. Refused when read, through a worker too.
    let bytes = surfaceless_face_brep();
    assert_refused(&bytes, "a face with no surface", "no surface");
    let err = kernel()
        .summarize(&ShapeData::unchecked(Blob::inline(bytes)))
        .unwrap_err();
    assert_eq!(err.code(), silico_core::ErrorCode::InvalidInput, "{err:#?}");
}

/// The rotation of the quaternion (w, x, y, z), whose entries are ratios of small integers
/// (no platform math), with a move after it.
fn rotation(q: [i32; 4], offset: [f64; 3]) -> [[f64; 4]; 3] {
    let [w, x, y, z] = q.map(f64::from);
    let n = w * w + x * x + y * y + z * z;
    [
        [
            (w * w + x * x - y * y - z * z) / n,
            2.0 * (x * y - w * z) / n,
            2.0 * (x * z + w * y) / n,
            offset[0],
        ],
        [
            2.0 * (x * y + w * z) / n,
            (w * w - x * x + y * y - z * z) / n,
            2.0 * (y * z - w * x) / n,
            offset[1],
        ],
        [
            2.0 * (x * z - w * y) / n,
            2.0 * (y * z + w * x) / n,
            (w * w - x * x - y * y + z * z) / n,
            offset[2],
        ],
    ]
}

/// The inverse of `rows`' rotation applied to `v`: its transpose.
fn unapply(rows: [[f64; 4]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|k| rows[0][k] * v[0] + rows[1][k] * v[1] + rows[2][k] * v[2])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn a_turned_primitives_normals_are_its_surfaces_turned(
        primitive in primitive(),
        q in prop::array::uniform4(-4i32..=4).prop_filter("a rotation", |q| q.iter().any(|&c| c != 0)),
        offset in prop::array::uniform3(prop_oneof![Just(0.0), Just(1e5), -100.0f64..100.0]),
    ) {
        let Ok(shape) = make(&primitive) else {
            return Ok(());
        };
        let size = extent(&primitive);
        let rows = rotation(q, offset);
        let turned = relocated(&shape, rows);
        let mesh = tessellate(&turned, size / 50.0).unwrap();
        assert_sound(&format!("{primitive:?} turned by {q:?}"), &mesh);
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            // Back where the primitive was made, the surface's own normal, turned.
            let local = unapply(rows, sub(*p, offset));
            let n = wide(*n);
            let tol = 1e-9 * size.max(1e5).max(1.0);
            if let Some(expected) = expected_normal(&primitive, local, unapply(rows, n), tol) {
                let expected = apply(rows, expected);
                prop_assert!(
                    apart(n, expected) <= 1e-6,
                    "{:?} turned by {:?}: {:?} at {:?}, expected {:?}", primitive, q, n, p, expected
                );
            }
        }
    }
}

#[test]
fn a_huge_balls_mesh_is_refused_or_sound() {
    // A ball of radius 1e100 (by rewriting every 5 in a radius-5 ball's B-rep): its
    // |dP/du × dP/dv| = r² cos v squared overflows, and OCCT throws ("gp_Dir() - input
    // vector has zero norm") before any normal is stored. Either a structured error or unit
    // normals, never a crash. (No input found gives finite points and normals that aren't;
    // the core's Mesh::validate stands guard against them.)
    let ball = to_brep(&sphere(5.0)).unwrap();
    let body = &ball[..ball.len() - 4];
    let five = 5.0f64.to_le_bytes();
    let mut patched = body.to_vec();
    let mut i = 0;
    let mut count = 0;
    while i + 8 <= patched.len() {
        if patched[i..i + 8] == five {
            patched[i..i + 8].copy_from_slice(&1e100f64.to_le_bytes());
            count += 1;
            i += 8;
        } else {
            i += 1;
        }
    }
    assert!(count > 0);
    let huge = from_brep(&with_crc(patched)).unwrap();
    // Through a worker, so a crash is seen as one.
    let mut worker = kernel();
    let data = ShapeData::unchecked(Blob::inline(to_brep(&huge).unwrap()));
    match worker.tessellate(&data, 1e97) {
        Err(e) => {
            assert!(e.suggestion().is_some(), "{e:?}");
            assert_ne!(e.code(), silico_core::ErrorCode::ProtocolError, "{e:?}");
        }
        Ok(mesh) => {
            for n in &mesh.normals {
                assert!((norm(wide(*n)) - 1.0).abs() <= 1e-6, "{n:?}");
            }
        }
    }
    // The worker is still good for the next request.
    let data = ShapeData::unchecked(Blob::inline(to_brep(&sphere(5.0)).unwrap()));
    assert!(worker.tessellate(&data, DEFLECTION).is_ok());
}

// ----------------------------------------------------------------------------------------
// The mesh's own checks, on meshes no worker should send.

fn triangle_mesh() -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        triangles: vec![[0, 1, 2]],
        face_triangles: vec![1],
        edge_points: vec![],
        edge_point_counts: vec![],
    }
}

#[test]
fn a_mesh_with_normals_that_arent_numbers_is_refused() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for k in 0..3 {
            let mut mesh = triangle_mesh();
            mesh.normals[1][k] = bad;
            let err = mesh.validate().unwrap_err();
            assert!(err.contains("normal"), "{bad} at {k}: {err}");
        }
    }
}

#[test]
fn a_mesh_with_a_zero_or_unscaled_normal_is_refused() {
    // Mesh::validate, which the core runs on a worker's mesh before anything uses it,
    // refuses normals of length 0, 2, 1e-30, and f32::MAX: the field promises the unit
    // normal, and normalizing (0, 0, 0) for shading gives NaN. It accepted them until
    // /break-it found it.
    for bad in [
        [0.0f32, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [1e-30, 0.0, 0.0],
        [f32::MAX, 0.0, 0.0],
    ] {
        let mut mesh = triangle_mesh();
        mesh.normals[0] = bad;
        assert!(mesh.validate().is_err(), "{bad:?} was accepted");
    }
}

/// What each triangle's face is, counted out the slow way.
fn faces_by_hand(triangles: usize, counts: &[u32]) -> Vec<u32> {
    let mut faces = Vec::new();
    for (face, &count) in counts.iter().enumerate() {
        for _ in 0..count {
            if faces.len() == triangles {
                return faces;
            }
            faces.push(face as u32);
        }
    }
    faces
}

fn counts() -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(
        prop_oneof![
            4 => Just(0u32),
            4 => 1u32..4,
            1 => Just(u32::MAX),
            1 => Just(u32::MAX - 1),
            2 => 0u32..40,
        ],
        0..12,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn each_triangles_face_is_counted_out_in_order(
        counts in counts(),
        extra in prop_oneof![Just(0usize), 0usize..5],
        short in prop_oneof![Just(0usize), 0usize..5],
    ) {
        // Triangles to match the counts where they're small, then a few more or fewer.
        let total: usize = counts.iter().map(|&c| c.min(40) as usize).sum();
        let triangles = (total + extra).saturating_sub(short);
        let mut mesh = triangle_mesh();
        mesh.triangles = vec![[0, 1, 2]; triangles];
        mesh.face_triangles = counts.clone();
        let faces = mesh.triangle_faces();
        prop_assert_eq!(&faces, &faces_by_hand(triangles, &counts));
        prop_assert!(faces.len() <= triangles);
        if mesh.validate().is_ok() {
            // A sound mesh: every triangle has a face.
            prop_assert_eq!(faces.len(), triangles);
        }
    }
}

#[test]
fn many_faces_of_no_triangles_cost_little() {
    // A hostile reply's counts: ten million faces, each claiming u32::MAX triangles, with
    // none sent. The faces' list is empty, at once.
    let mut mesh = triangle_mesh();
    mesh.triangles.clear();
    mesh.face_triangles = vec![u32::MAX; 10_000_000];
    // Counting them out one by one would allocate u32::MAX entries per face, and fail.
    assert!(mesh.triangle_faces().is_empty());
}

#[test]
fn a_text_brep_is_refused_as_a_binary_one_is() {
    // OCCT's text B-rep (FreeCAD's exportBrep), read for the corpus of real parts, goes
    // through the same checks: the text of meshonly.bin's face is refused for the same
    // reason. Text that isn't a B-rep is an error. (Some corrupt text crashes OCCT's
    // reader, or never returns: it's read only in silico-brep-convert's own process.)
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brep");
    let text = std::fs::read(dir.join("meshonly.brep")).unwrap();
    let err = silico_kernel::from_text_brep(&text).err().unwrap();
    assert!(err.message().contains("no surface"), "{err:?}");
    for bad in [&b""[..], b"not a B-rep at all", &text[..text.len() / 2]] {
        assert!(silico_kernel::from_text_brep(bad).is_err());
    }
}

#[test]
fn the_converter_writes_what_the_reader_reads() {
    // silico-brep-convert: the text of a part on stdin, Silico's B-rep on stdout; a part
    // Silico would refuse is refused, with why on stderr and a failing exit.
    use std::io::Write as _;
    let convert = |text: &[u8]| {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_silico-brep-convert"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(text).unwrap();
        child.wait_with_output().unwrap()
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brep");
    let out = convert(&std::fs::read(dir.join("meshonly.brep")).unwrap());
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no surface"),
        "{out:?}"
    );
    let out = convert(b"garbage");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("silico-brep-convert: "));
}

#[test]
fn a_valid_solid_with_faces_too_small_to_mesh_is_meshed_without_them() {
    // Every edge of a 10 × 20 × 30 box chamfered at 4.99999999: a valid solid of 26 faces,
    // 8 of them slivers of a few 1e-7 mm² that OCCT's mesher leaves unmeshed. As FreeCAD
    // gives such faces empty domains, they get no triangles, and the rest is meshed: the
    // mesh's area is the solid's, but for the slivers.
    let block = silico_kernel::make_box(10.0, 20.0, 30.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let built = silico_kernel::chamfer(
        &block,
        &all,
        silico_protocol::ChamferSize::Equal {
            distance: 4.999_999_99,
        },
        false,
    )
    .unwrap();
    let summary = silico_kernel::summarize(&built.shape).unwrap();
    assert!(summary.valid);
    let mesh = silico_kernel::tessellate(&built.shape, 0.01).unwrap();
    mesh.validate().unwrap();
    assert_eq!(mesh.face_triangles.len(), summary.faces as usize);
    // Exactly the slivers, as FreeCAD finds them: Face8, 9, 11, 13, 14, 15, 17, and 19.
    let empty: Vec<usize> = (0..mesh.face_triangles.len())
        .filter(|&i| mesh.face_triangles[i] == 0)
        .map(|i| i + 1)
        .collect();
    assert_eq!(empty, [8, 9, 11, 13, 14, 15, 17, 19]);
    let area = mesh_area(&mesh);
    assert!(
        (area - summary.area).abs() <= 1e-4,
        "{area} {}",
        summary.area
    );
}

#[test]
fn a_face_that_cant_be_meshed_on_a_shape_that_isnt_valid_is_refused_naming_it() {
    // A face whose edges cross (a bowtie, as FreeCAD writes it): OCCT can't mesh it, and on a
    // shape that isn't valid that's refused, not left out.
    let text: String = serde_json::from_value(oracle::freecad(
        "import json, Part, FreeCAD\nV = FreeCAD.Vector\n\
         w = Part.makePolygon([V(0, 0, 0), V(10, 10, 0), V(10, 0, 0), V(0, 10, 0), V(0, 0, 0)])\n\
         print('SILICO_ORACLE ' + json.dumps(Part.Face(Part.Plane(), w).exportBrepToString()))\n",
    ))
    .unwrap();
    let bowtie = silico_kernel::from_text_brep(text.as_bytes()).unwrap();
    for deflection in [0.01, 1.0] {
        let err = silico_kernel::tessellate(&bowtie, deflection).unwrap_err();
        assert_eq!(err.message(), "Face1 couldn't be meshed", "{err:?}");
        assert_eq!(err.element(), Some("Face1"));
        assert!(err.suggestion().unwrap().contains("rebuild"), "{err:?}");
    }
}

#[test]
fn a_valid_solid_with_slivers_meshes_at_every_deflection_silico_allows() {
    // A 1 × 2 × 3 box chamfered nearly through: slivers of 4e-7 mm², too narrow for OCCT's
    // tolerance at any deflection. Down to the finest deflection Silico allows, the mesh
    // leaves them out rather than refusing (whether OCCT can mesh a sliver has nothing to do
    // with the deflection).
    let block = silico_kernel::make_box(1.0, 2.0, 3.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let built = silico_kernel::chamfer(
        &block,
        &all,
        silico_protocol::ChamferSize::Equal {
            distance: 0.499_999_9,
        },
        false,
    )
    .unwrap();
    let summary = silico_kernel::summarize(&built.shape).unwrap();
    assert!(summary.valid);
    let floor = 14.0f64.sqrt() * silico_kernel::MIN_RELATIVE_DEFLECTION;
    for deflection in [0.01, 0.001, floor * 1.001] {
        let mesh = silico_kernel::tessellate(&built.shape, deflection)
            .unwrap_or_else(|e| panic!("deflection {deflection}: {e:?}"));
        // Exactly the slivers OCCT can't mesh, as FreeCAD finds them: Face8 and Face13.
        let empty: Vec<usize> = (0..mesh.face_triangles.len())
            .filter(|&i| mesh.face_triangles[i] == 0)
            .map(|i| i + 1)
            .collect();
        assert_eq!(empty, [8, 13], "deflection {deflection}");
        let area = mesh_area(&mesh);
        assert!(
            (area - summary.area).abs() <= 1e-5,
            "deflection {deflection}: {area} {}",
            summary.area
        );
    }
}

#[test]
fn a_valid_film_meshes_without_its_side_faces_at_any_deflection() {
    // A 10 × 20 rectangle extruded 1e-6: valid, and its four side faces (1e-5 and 2e-5 mm²)
    // too thin for OCCT's tolerance. At any deflection they're left out, not refused; the
    // faces OCCT meshes are meshed.
    let profile = Profile {
        points: vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 20.0, 0.0],
            [0.0, 20.0, 0.0],
        ],
    };
    let film = silico_kernel::extrude(&profile, 1e-6, None).unwrap().shape;
    assert!(summarize(&film).unwrap().valid);
    for deflection in [0.003, 0.004, 0.005, 0.1] {
        let mesh = tessellate(&film, deflection).unwrap();
        // The four sides (Face1 to Face4) are left out; the two 10 × 20 ends are meshed.
        assert_eq!(mesh.face_triangles.len(), 6, "{deflection}");
        assert_eq!(mesh.face_triangles[..4], [0, 0, 0, 0], "{deflection}");
        assert!(
            mesh.face_triangles[4..].iter().all(|&n| n > 0),
            "{deflection}"
        );
        assert!((mesh_area(&mesh) - 400.0).abs() <= 1e-9, "{deflection}");
    }
}

/// The sum of a mesh's triangles' areas.
fn mesh_area(mesh: &Mesh) -> f64 {
    mesh.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            norm(cross(sub(b, a), sub(c, a))) / 2.0
        })
        .sum()
}
