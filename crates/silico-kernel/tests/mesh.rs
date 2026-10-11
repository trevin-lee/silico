//! Display meshes: closed and facing outward (checked by the volume they enclose), edges
//! numbered as the fillet numbers them, and the same every time; each triangle's face, and
//! each vertex's normal, which is its surface's, pointing out of the solid
//! (docs/ports/tessellation.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/mesh_geometry.rs"]
mod geometry;
#[path = "common/imports.rs"]
mod imports;
#[path = "common/numbers.rs"]
mod numbers;

use geometry::{apart, cross, cylinder, dot, faces, norm, sub, wide};
use numbers::relative_error;
use std::collections::BTreeMap;

use silico_kernel::{
    BooleanOp, KernelErrorKind, Mesh, Primitive, Shape, boolean, fillet, make, make_box,
    tessellate, translate,
};

/// The volume a closed mesh encloses (divergence theorem), taken about `o`, a point inside
/// the solid: every triangle then adds a positive tetrahedron if it faces outward, so a
/// missing or flipped triangle changes the result. (About a point on a face's plane, as
/// the origin is for half a box's faces, that face's triangles would add nothing either
/// way.)
fn volume_about(mesh: &Mesh, o: [f64; 3]) -> f64 {
    mesh.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| sub(mesh.positions[i as usize], o));
            dot(a, cross(b, c)) / 6.0
        })
        .sum()
}

const INSIDE_THE_BOX: [f64; 3] = [5.0, 10.0, 15.0];

fn area(mesh: &Mesh) -> f64 {
    mesh.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            let n = cross(sub(b, a), sub(c, a));
            dot(n, n).sqrt() / 2.0
        })
        .sum()
}

#[test]
fn a_box_mesh_is_closed_and_faces_outward() {
    let mesh = tessellate(&make_box(10.0, 20.0, 30.0).unwrap(), 0.1).unwrap();
    // Flat faces mesh exactly: the enclosed volume and the area are the box's.
    assert!(
        relative_error(volume_about(&mesh, INSIDE_THE_BOX), 6000.0) <= 1e-12,
        "{}",
        volume_about(&mesh, INSIDE_THE_BOX)
    );
    assert!(
        relative_error(area(&mesh), 2200.0) <= 1e-12,
        "{}",
        area(&mesh)
    );
    assert_eq!(mesh.face_triangles.len(), 6);
    assert_eq!(
        mesh.face_triangles.iter().sum::<u32>() as usize,
        mesh.triangles.len()
    );
    assert!(
        mesh.triangles
            .iter()
            .flatten()
            .all(|&i| (i as usize) < mesh.positions.len())
    );
}

#[test]
fn edges_are_numbered_as_the_fillet_numbers_them() {
    // FreeCAD's Edge1 of this box runs from (0, 0, 30) to (0, 0, 0); the fillet tests rely
    // on the same numbering.
    let mesh = tessellate(&make_box(10.0, 20.0, 30.0).unwrap(), 0.1).unwrap();
    let edges: Vec<_> = mesh.edges().collect();
    assert_eq!(edges.len(), 12);
    let edge1 = edges[0];
    let ends = [edge1[0], edge1[edge1.len() - 1]];
    assert!(
        ends.contains(&[0.0, 0.0, 30.0]) && ends.contains(&[0.0, 0.0, 0.0]),
        "{ends:?}"
    );
}

#[test]
fn every_box_edge_polyline_is_its_own_box_edge() {
    // Each of the twelve polylines runs between two corners that differ in one coordinate
    // (a box edge), and no two are the same edge.
    let mesh = tessellate(&make_box(10.0, 20.0, 30.0).unwrap(), 0.1).unwrap();
    let corner = |p: [f64; 3]| {
        assert!(
            (p[0] == 0.0 || p[0] == 10.0)
                && (p[1] == 0.0 || p[1] == 20.0)
                && (p[2] == 0.0 || p[2] == 30.0),
            "{p:?} isn't a corner"
        );
        p.map(|c| c as i64)
    };
    let mut seen = std::collections::BTreeSet::new();
    for (i, edge) in mesh.edges().enumerate() {
        let (a, b) = (corner(edge[0]), corner(edge[edge.len() - 1]));
        assert_eq!(
            (0..3).filter(|&k| a[k] != b[k]).count(),
            1,
            "Edge{} isn't a box edge",
            i + 1
        );
        assert!(seen.insert((a.min(b), a.max(b))), "Edge{} repeats", i + 1);
    }
    assert_eq!(seen.len(), 12);
}

#[test]
fn a_filleted_box_mesh_approximates_the_solid() {
    let rounded = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
        .unwrap()
        .shape;
    let mesh = tessellate(&rounded, 0.01).unwrap();
    // Analytic volume; the mesh's chords cut the curved face, so it's slightly less, and
    // within what a 0.01 deflection allows over the fillet's 2π·2/4·30 ≈ 94 area.
    let volume = 6000.0 - (4.0 - std::f64::consts::PI) * 30.0;
    let meshed = volume_about(&mesh, INSIDE_THE_BOX);
    assert!(
        meshed <= volume && volume - meshed < 0.01 * 94.3,
        "{meshed} vs {volume}"
    );
    assert_eq!(mesh.face_triangles.len(), 7);
    assert_eq!(mesh.edges().count(), 15);
    // The fillet's arc edges stay within the deflection of the true circle: radius 2 about
    // the line x = 2, y = 2 (Edge1 of the box ran along x = 0, y = 0).
    let off_circle = mesh
        .edges()
        .flatten()
        .filter(|p| p[0] < 2.0 && p[1] < 2.0)
        .map(|p| {
            let (dx, dy) = (p[0] - 2.0, p[1] - 2.0);
            ((dx * dx + dy * dy).sqrt() - 2.0).abs()
        })
        .fold(0.0, f64::max);
    assert!(off_circle <= 1e-6, "{off_circle}");
}

#[test]
fn a_mesh_is_the_same_every_time() {
    // Two shapes built separately (OCCT keeps a shape's mesh, so meshing one shape twice
    // would prove nothing).
    let mesh = || {
        let rounded = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1, 2], 1.5)
            .unwrap()
            .shape;
        tessellate(&rounded, 0.05).unwrap()
    };
    assert_eq!(mesh(), mesh());
}

#[test]
fn a_bad_deflection_is_invalid_input() {
    let b = make_box(1.0, 1.0, 1.0).unwrap();
    for deflection in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let err = tessellate(&b, deflection).unwrap_err();
        assert_eq!(
            err.kind(),
            KernelErrorKind::InvalidInput,
            "{deflection}: {err}"
        );
    }
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}

#[test]
fn meshing_leaves_the_shape_as_it_was() {
    // OCCT stores a mesh on the shape it meshes; Silico removes it, and writes B-reps
    // without meshes, so meshing for display changes nothing that's stored or built on.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let before = silico_kernel::to_brep(&b).unwrap();
    tessellate(&b, 0.1).unwrap();
    assert_eq!(
        silico_kernel::to_brep(&b).unwrap(),
        before,
        "the B-rep changed"
    );
    let fillet_of =
        |shape| silico_kernel::to_brep(&fillet(shape, &[1], 2.0).unwrap().shape).unwrap();
    assert_eq!(
        fillet_of(&b),
        fillet_of(&make_box(10.0, 20.0, 30.0).unwrap())
    );
    // A coarse mesh after a fine one is the coarse mesh, not the fine one kept.
    let rounded = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
        .unwrap()
        .shape;
    tessellate(&rounded, 0.01).unwrap();
    let coarse_after = tessellate(&rounded, 1.0).unwrap();
    let fresh = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0)
        .unwrap()
        .shape;
    assert_eq!(coarse_after, tessellate(&fresh, 1.0).unwrap());
}

#[test]
fn a_deflection_too_fine_for_the_shape_is_refused() {
    // The box's diagonal is √1400 ≈ 37.4, so the floor is about 0.0037.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let err = tessellate(&b, 1e-5).unwrap_err();
    assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{err}");
    assert!(err.message().contains("too fine"), "{err}");
    // The floor, readable: about √1400 × 1e-4 (OCCT's box is a hair larger).
    assert!(err.message().contains("use at least 0.003741657"), "{err}");
    assert!(tessellate(&b, 0.004).is_ok());
}

/// A 20 × 20 × 10 box with a hole of radius 5 through it, about the vertical axis through
/// (10, 10).
fn box_with_a_hole() -> Shape {
    let tool = translate(&cylinder(5.0, 12.0), [10.0, 10.0, -1.0]).unwrap();
    boolean(
        BooleanOp::Cut,
        &make_box(20.0, 20.0, 10.0).unwrap(),
        &tool.shape,
    )
    .unwrap()
    .shape
}

#[test]
fn box_mesh() {
    // Two triangles on each of six faces, the faces numbered 0 to 5, every vertex inside
    // the box, and every normal the outward axis of its face: -x on the face at x = 0, +x
    // at x = 10, and so on.
    let mesh = tessellate(&make_box(10.0, 20.0, 30.0).unwrap(), 0.1).unwrap();
    assert_eq!(mesh.validate(), Ok(()));
    assert_eq!(mesh.triangles.len(), 12);
    assert_eq!(mesh.triangle_faces(), [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
    let size = [10.0, 20.0, 30.0];
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        assert!((0..3).all(|k| (0.0..=size[k]).contains(&p[k])), "{p:?}");
        let axis = (0..3).find(|&k| n[k] != 0.0).unwrap();
        let mut expected = [0.0f32; 3];
        expected[axis] = if p[axis] == 0.0 { -1.0 } else { 1.0 };
        assert_eq!(*n, expected, "at {p:?}");
        assert!(p[axis] == 0.0 || p[axis] == size[axis], "{p:?}");
    }
}

/// Every edge between two triangles, with vertices joined where their coordinates are
/// equal (each face has its own vertices), and how many triangles share it.
fn shared_edges(mesh: &Mesh) -> BTreeMap<(usize, usize), u32> {
    let mut joined: BTreeMap<[u64; 3], usize> = BTreeMap::new();
    let ids: Vec<usize> = mesh
        .positions
        .iter()
        .map(|p| {
            let next = joined.len();
            *joined.entry(p.map(f64::to_bits)).or_insert(next)
        })
        .collect();
    let mut edges = BTreeMap::new();
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| ids[i as usize]);
        assert!(a != b && b != c && c != a, "a degenerate triangle: {t:?}");
        for (u, v) in [(a, b), (b, c), (c, a)] {
            *edges.entry((u.min(v), u.max(v))).or_insert(0) += 1;
        }
    }
    edges
}

#[test]
fn closed_solid_mesh_watertight() {
    // Neighbouring faces share their edge's points exactly, so the mesh closes: every edge
    // has a triangle on each side.
    for (what, shape) in [
        ("box", make_box(10.0, 20.0, 30.0).unwrap()),
        ("cylinder", cylinder(5.0, 10.0)),
        ("box minus cylinder", box_with_a_hole()),
    ] {
        let mesh = tessellate(&shape, 0.05).unwrap();
        let open: Vec<_> = shared_edges(&mesh)
            .into_iter()
            .filter(|&(_, n)| n != 2)
            .collect();
        assert!(open.is_empty(), "{what}: edges not shared by two: {open:?}");
    }
}

#[test]
fn cylinder_deflection_respected() {
    // The side of a cylinder of radius 5: every vertex on the surface, every chord between
    // neighbouring vertices within the deflection of it (its sagitta, 5 less its midpoint's
    // distance from the axis), and every normal radial. The ends' normals are the axis.
    let deflection = 0.05;
    let mesh = tessellate(&cylinder(5.0, 10.0), deflection).unwrap();
    let on_side = |t: &[u32; 3]| t.iter().all(|&i| mesh.normals[i as usize][2] == 0.0);
    let mut sides = 0;
    let mut worst = 0.0f64;
    for face in faces(&mesh) {
        if !face.iter().all(on_side) {
            for i in face.iter().flatten() {
                let (p, n) = (mesh.positions[*i as usize], mesh.normals[*i as usize]);
                assert_eq!(n, [0.0, 0.0, if p[2] == 0.0 { -1.0 } else { 1.0 }], "{p:?}");
            }
            continue;
        }
        sides += 1;
        for t in &face {
            for i in t {
                let (p, n) = (mesh.positions[*i as usize], mesh.normals[*i as usize]);
                let r = norm([p[0], p[1], 0.0]);
                assert!((r - 5.0).abs() <= 1e-12, "{p:?} is {r} from the axis");
                let radial = [p[0] / r, p[1] / r, 0.0];
                assert!(apart(wide(n), radial) <= 1e-6, "{n:?} at {p:?}");
            }
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (mesh.positions[a as usize], mesh.positions[b as usize]);
                let middle = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, 0.0];
                worst = worst.max(5.0 - norm(middle));
            }
        }
    }
    assert_eq!(sides, 1);
    assert!(worst > 0.0 && worst <= deflection, "sagitta {worst}");
}

#[test]
fn a_spheres_normals_point_from_its_center() {
    let mesh = tessellate(&make(&Primitive::Sphere { radius: 5.0 }).unwrap(), 0.05).unwrap();
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let out = p.map(|c| c / norm(*p));
        assert!(apart(wide(*n), out) <= 1e-6, "{n:?} at {p:?}");
    }
}

#[test]
fn a_holes_normals_point_toward_its_axis() {
    // The hole's face is the cylinder's surface reversed: its normals point out of the
    // solid, into the hole, toward the axis through (10, 10).
    let mesh = tessellate(&box_with_a_hole(), 0.05).unwrap();
    let mut on_hole = 0;
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let from_axis = [p[0] - 10.0, p[1] - 10.0, 0.0];
        if (norm(from_axis) - 5.0).abs() > 1e-9 || n[2] != 0.0 {
            continue;
        }
        on_hole += 1;
        let inward = from_axis.map(|c| -c / norm(from_axis));
        assert!(apart(wide(*n), inward) <= 1e-6, "{n:?} at {p:?}");
    }
    assert!(on_hole > 0);
}

#[test]
fn every_triangle_winds_as_its_normals_point() {
    // Seen from where its vertices' normals point, every triangle winds counterclockwise:
    // the two agree on which side is out. On flat faces, curved ones, reversed ones, and
    // fillets. (A sphere's or torus's mesh has triangles of no area where a degenerate
    // edge, a pole, is a single point: they wind no way, and can't be seen.)
    let shapes = [
        make_box(10.0, 20.0, 30.0).unwrap(),
        cylinder(5.0, 10.0),
        make(&Primitive::Sphere { radius: 5.0 }).unwrap(),
        make(&Primitive::Torus {
            major_radius: 10.0,
            minor_radius: 2.0,
        })
        .unwrap(),
        box_with_a_hole(),
        fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1, 2, 3], 2.0)
            .unwrap()
            .shape,
    ];
    for shape in &shapes {
        let mesh = tessellate(shape, 0.05).unwrap();
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            let facing = cross(sub(b, a), sub(c, a));
            if facing == [0.0; 3] {
                continue;
            }
            let normals = t.map(|i| wide(mesh.normals[i as usize]));
            for n in normals {
                assert!(dot(facing, n) > 0.0, "{t:?}: {facing:?} against {n:?}");
            }
        }
    }
}
