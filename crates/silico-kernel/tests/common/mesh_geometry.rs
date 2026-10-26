//! Vector arithmetic on points and normals, rotations by hand, and for the mesh tests a
//! mesh's triangles by face and a cylinder to mesh.

use silico_kernel::{Mesh, Primitive, Shape, make};

pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn norm(v: [f64; 3]) -> f64 {
    dot(v, v).sqrt()
}

/// Rodrigues: the rotation by `degrees` about `axis` (not unit), as a matrix.
pub fn rotation(axis: [f64; 3], degrees: f64) -> [[f64; 3]; 3] {
    let n = norm(axis);
    let [x, y, z] = axis.map(|a| a / n);
    let (s, c) = (
        libm::sin(degrees.to_radians()),
        libm::cos(degrees.to_radians()),
    );
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// `p` turned by `r`, then moved by `t`.
pub fn apply(r: &[[f64; 3]; 3], p: [f64; 3], t: [f64; 3]) -> [f64; 3] {
    let mut out = t;
    for (i, row) in r.iter().enumerate() {
        out[i] += dot(*row, p);
    }
    out
}

/// A normal, widened for arithmetic.
pub fn wide(n: [f32; 3]) -> [f64; 3] {
    n.map(f64::from)
}

/// The distance between two directions.
pub fn apart(a: [f64; 3], b: [f64; 3]) -> f64 {
    norm(sub(a, b))
}

/// The triangles of each face, as vertex indices.
pub fn faces(mesh: &Mesh) -> Vec<Vec<[u32; 3]>> {
    let mut faces = vec![Vec::new(); mesh.face_triangles.len()];
    for (t, face) in mesh.triangles.iter().zip(mesh.triangle_faces()) {
        faces[face as usize].push(*t);
    }
    faces
}

pub fn cylinder(radius: f64, height: f64) -> Shape {
    make(&Primitive::Cylinder { radius, height }).unwrap()
}
