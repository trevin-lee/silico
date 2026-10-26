//! For the tests of a point's distance from an element (`silico_kernel::point_distance`): a
//! cube at the origin, and its edges and vertices found by where they are.

use silico_kernel::{Primitive, Shape, describe_whole, make};
use silico_protocol::{ElementKind, ElementRef};

/// A cube of `side` at the origin: Face1 x = 0, Face2 x = side, Face3 y = 0, Face4 y = side,
/// Face5 z = 0, Face6 z = side.
pub fn cube(side: f64) -> Shape {
    make(&Primitive::Box {
        dx: side,
        dy: side,
        dz: side,
    })
    .unwrap()
}

/// The number of `shape`'s edge from `a` to `b`, either way along it.
pub fn edge_between(shape: &Shape, a: [f64; 3], b: [f64; 3]) -> ElementRef {
    let d = describe_whole(shape).unwrap();
    let i = d
        .edges
        .iter()
        .position(|e| (e.start == a && e.end == b) || (e.start == b && e.end == a))
        .unwrap();
    ElementRef::edge(u32::try_from(i + 1).unwrap())
}

/// The number of `shape`'s vertex at `at`.
pub fn vertex_at(shape: &Shape, at: [f64; 3]) -> ElementRef {
    let d = describe_whole(shape).unwrap();
    let i = d.vertices.iter().position(|v| *v == at).unwrap();
    ElementRef {
        kind: ElementKind::Vertex,
        index: u32::try_from(i + 1).unwrap(),
    }
}
