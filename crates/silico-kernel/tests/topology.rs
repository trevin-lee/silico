//! M2.7: the result's topology, which naming reads with each history: each face's edges and
//! outer edges, each edge's vertices. Expected values are worked by hand from the shapes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_kernel::{BooleanOp, Primitive, boolean, make, translate};

#[test]
fn a_holes_rim_bounds_its_face_but_isnt_in_its_outer_wire() {
    // A 10 mm cube with a cylinder (r 2) cut straight through it, top to bottom.
    let cube = make(&Primitive::Box {
        dx: 10.0,
        dy: 10.0,
        dz: 10.0,
    })
    .unwrap();
    let pin = make(&Primitive::Cylinder {
        radius: 2.0,
        height: 20.0,
    })
    .unwrap();
    let pin = translate(&pin, [5.0, 5.0, -5.0]).unwrap().shape;
    let cut = boolean(BooleanOp::Cut, &cube, &pin).unwrap();
    let h = &cut.history;
    let t = &h.topology;
    // Six box faces and the hole's wall; the box's 12 edges, two rims and the wall's seam;
    // the box's 8 corners and a vertex on each rim.
    assert_eq!(
        (h.result.faces, h.result.edges, h.result.vertices),
        (7, 15, 10)
    );
    // Per face, how many edges bound it and how many its outer wire has: the top and the
    // bottom have four outer edges and a rim; the sides have four; the wall has its two
    // rims and its seam, all in its one wire.
    let mut shapes: Vec<(usize, usize)> = t
        .face_edges
        .iter()
        .zip(&t.outer_edges)
        .map(|(all, outer)| (all.len(), outer.len()))
        .collect();
    shapes.sort_unstable();
    assert_eq!(
        shapes,
        [(3, 3), (4, 4), (4, 4), (4, 4), (4, 4), (5, 4), (5, 4)]
    );
    // The two rims: the edges in some face's list but not in its outer wire.
    let mut rims: Vec<u32> = t
        .face_edges
        .iter()
        .zip(&t.outer_edges)
        .flat_map(|(all, outer)| all.iter().filter(|e| !outer.contains(e)).copied())
        .collect();
    rims.sort_unstable();
    assert_eq!(rims.len(), 2);
    // A rim is closed: one vertex. The seam joins the two rims' vertices; a box edge joins
    // two corners.
    let wall = t.face_edges.iter().position(|e| e.len() == 3).unwrap();
    let seam = *t.face_edges[wall]
        .iter()
        .find(|e| !rims.contains(e))
        .unwrap();
    for rim in &rims {
        assert_eq!(t.edge_vertices[*rim as usize - 1].len(), 1);
    }
    let rim_vertices: Vec<u32> = rims
        .iter()
        .map(|r| t.edge_vertices[*r as usize - 1][0])
        .collect();
    let mut seam_vertices = t.edge_vertices[seam as usize - 1].clone();
    seam_vertices.sort_unstable();
    let mut expected = rim_vertices.clone();
    expected.sort_unstable();
    assert_eq!(seam_vertices, expected);
    for (n, vertices) in t.edge_vertices.iter().enumerate() {
        let e = n as u32 + 1;
        if e != seam && !rims.contains(&e) {
            assert_eq!(vertices.len(), 2, "Edge{e}");
            assert!(
                vertices.iter().all(|v| !rim_vertices.contains(v)),
                "Edge{e}"
            );
        }
    }
    // A closed solid: every edge bounds exactly two faces (the seam its one face, twice,
    // which a face's list counts once).
    for e in 1..=h.result.edges {
        let faces = t.face_edges.iter().filter(|f| f.contains(&e)).count();
        assert_eq!(faces, if e == seam { 1 } else { 2 }, "Edge{e}");
    }
}

#[test]
fn a_moves_topology_is_its_inputs() {
    // A move changes no element's number, so the moved box's topology is OCCT's box's:
    // Face1 (x = 0) is bounded by Edges 1 to 4, and Edge1 runs from Vertex1 to Vertex2.
    let cube = make(&Primitive::Box {
        dx: 1.0,
        dy: 2.0,
        dz: 3.0,
    })
    .unwrap();
    let moved = translate(&cube, [1.0, 2.0, 3.0]).unwrap();
    let t = &moved.history.topology;
    assert_eq!(t.face_edges[0], [1, 2, 3, 4]);
    assert_eq!(t.edge_vertices[0], [1, 2]);
    assert_eq!(t.face_edges.len(), 6);
    assert!(t.face_edges.iter().all(|f| f.len() == 4));
    assert_eq!(
        t.outer_edges,
        t.face_edges
            .iter()
            .map(|f| {
                let mut f = f.clone();
                f.sort_unstable();
                f
            })
            .collect::<Vec<_>>()
    );
}
