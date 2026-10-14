//! A face found by where it is, for tests that need a face's number on a shape whose
//! numbering they don't derive.

use silico_kernel::{Shape, tessellate};

/// The number of the one face of `shape` whose mesh points all satisfy `on`.
pub fn face_where(shape: &Shape, on: impl Fn([f64; 3]) -> bool) -> u32 {
    let mesh = tessellate(shape, 0.01).unwrap();
    let mut triangles = mesh.triangles.iter();
    let mut found = Vec::new();
    for (i, &count) in mesh.face_triangles.iter().enumerate() {
        let mut all = true;
        for t in triangles.by_ref().take(count as usize) {
            all &= t.iter().all(|&v| on(mesh.positions[v as usize]));
        }
        if all {
            found.push(i as u32 + 1);
        }
    }
    assert_eq!(found.len(), 1, "faces matching: {found:?}");
    found[0]
}
