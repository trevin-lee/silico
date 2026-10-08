//! Display meshes: closed and facing outward (checked by the volume they enclose), edges
//! numbered as the fillet numbers them, and the same every time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/imports.rs"]
mod imports;

use silico_kernel::{Mesh, fillet, make_box, tessellate};

fn relative_error(actual: f64, expected: f64) -> f64 {
    ((actual - expected) / expected).abs()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

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
    assert_eq!(mesh.edges.len(), 12);
    let edge1 = &mesh.edges[0];
    let ends = [edge1[0], edge1[edge1.len() - 1]];
    assert!(
        ends.contains(&[0.0, 0.0, 30.0]) && ends.contains(&[0.0, 0.0, 0.0]),
        "{ends:?}"
    );
}

#[test]
fn a_filleted_box_mesh_approximates_the_solid() {
    let rounded = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0).unwrap();
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
    assert_eq!(mesh.edges.len(), 15);
}

#[test]
fn a_mesh_is_the_same_every_time() {
    // Two shapes built separately (OCCT keeps a shape's mesh, so meshing one shape twice
    // would prove nothing).
    let mesh = || {
        let rounded = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1, 2], 1.5).unwrap();
        tessellate(&rounded, 0.05).unwrap()
    };
    assert_eq!(mesh(), mesh());
}

#[test]
fn a_bad_deflection_is_invalid_input() {
    let b = make_box(1.0, 1.0, 1.0).unwrap();
    for deflection in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let err = tessellate(&b, deflection).unwrap_err();
        assert!(err.is_invalid_input(), "{deflection}: {err}");
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
    let fillet_of = |shape| silico_kernel::to_brep(&fillet(shape, &[1], 2.0).unwrap()).unwrap();
    assert_eq!(
        fillet_of(&b),
        fillet_of(&make_box(10.0, 20.0, 30.0).unwrap())
    );
    // A coarse mesh after a fine one is the coarse mesh, not the fine one kept.
    let rounded = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0).unwrap();
    tessellate(&rounded, 0.01).unwrap();
    let coarse_after = tessellate(&rounded, 1.0).unwrap();
    let fresh = fillet(&make_box(10.0, 20.0, 30.0).unwrap(), &[1], 2.0).unwrap();
    assert_eq!(coarse_after, tessellate(&fresh, 1.0).unwrap());
}

#[test]
fn a_deflection_too_fine_for_the_shape_is_refused() {
    // The box's diagonal is √1400 ≈ 37.4, so the floor is about 0.0037.
    let b = make_box(10.0, 20.0, 30.0).unwrap();
    let err = tessellate(&b, 1e-5).unwrap_err();
    assert!(
        err.is_invalid_input() && err.message().contains("too fine"),
        "{err}"
    );
    assert!(tessellate(&b, 0.004).is_ok());
}
