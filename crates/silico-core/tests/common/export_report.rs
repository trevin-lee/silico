//! For tests of the export core (silico_core::export): reports built by hand, as the kernel
//! worker reports a B-rep's meshed faces.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use silico_core::ErrorCode;
use silico_core::export::printable;
use silico_protocol::ExportMesh;

/// The unit tetrahedron's corners: P0 at the origin, P1, P2, P3 on the axes.
pub const P: [[f64; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
];

/// Its faces, each one triangle counterclockwise seen from outside: Face1 on z = 0, Face2
/// on y = 0, Face3 on x = 0, Face4 the slanted one.
pub const TETRA: [&[[usize; 3]]; 4] = [&[[0, 2, 1]], &[[0, 1, 3]], &[[0, 3, 2]], &[[1, 2, 3]]];

/// A one-solid, one-shell report of faces made of `triangles` over `points`, meshed at
/// 0.1 mm, as the worker reports a B-rep's: each face its own copy of the points it uses,
/// and an edge use for each side of its triangles, numbered by the two points it joins (lower
/// first, so its nodes run from the lower point's to the higher's), its vertices the
/// points', its deflection 0.1 − 1e-6.
pub fn report(points: &[[f64; 3]], faces: &[&[[usize; 3]]]) -> ExportMesh {
    let mut m = ExportMesh {
        tolerance: 0.1,
        deflection: 0.1,
        solids: 1,
        shell_solids: vec![0],
        shell_outer: vec![true],
        ..ExportMesh::default()
    };
    let mut edge_numbers: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for (f, triangles) in faces.iter().enumerate() {
        let base = m.nodes.len() as u32;
        let mut local: BTreeMap<usize, u32> = BTreeMap::new();
        let mut node = |p: usize, m: &mut ExportMesh| {
            *local.entry(p).or_insert_with(|| {
                m.nodes.push(points[p]);
                m.nodes.len() as u32 - 1
            })
        };
        let mut sides = BTreeSet::new();
        for t in *triangles {
            let corners = t.map(|p| node(p, &mut m));
            m.triangles.push(corners);
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                sides.insert((a.min(b), a.max(b)));
            }
        }
        m.face_nodes.push(m.nodes.len() as u32 - base);
        m.face_triangles.push(triangles.len() as u32);
        m.face_numbers.push(f as u32 + 1);
        m.face_deflections.push(0.0);
        m.face_shells.push(0);
        // OCCT's own tolerance, Precision::Confusion, as a box's faces carry it.
        m.face_tolerances.push(1e-7);
        for (a, b) in sides {
            let next = edge_numbers.len() as u32 + 1;
            let number = *edge_numbers.entry((a, b)).or_insert(next);
            m.edge_faces.push(f as u32);
            m.edge_numbers.push(number);
            m.edge_degenerate.push(false);
            m.edge_vertices.push([a as u32 + 1, b as u32 + 1]);
            m.edge_deflections.push(0.1 - 1e-6);
            m.edge_node_counts.push(2);
            let (na, nb) = (node(a, &mut m), node(b, &mut m));
            m.edge_nodes.extend([na, nb]);
        }
    }
    m.validate().unwrap();
    m
}

pub fn tetra() -> ExportMesh {
    report(&P, &TETRA)
}

/// `extra`'s faces appended to `base` as shell `shell` of solid `solid` (a new shell, and a
/// new solid if `solid` is past `base`'s), its edges and vertices renumbered past `base`'s
/// so the two share none.
pub fn with_shell(base: &ExportMesh, extra: &ExportMesh, solid: u32, outer: bool) -> ExportMesh {
    let mut m = base.clone();
    let offset = m.nodes.len() as u32;
    let faces = m.face_nodes.len() as u32;
    let shell = m.shell_solids.len() as u32;
    let edges = m.edge_numbers.iter().copied().max().unwrap_or(0);
    let vertices = m.edge_vertices.iter().flatten().copied().max().unwrap_or(0);
    m.shell_solids.push(solid);
    m.shell_outer.push(outer);
    m.solids = m.solids.max(solid + 1);
    m.nodes.extend(&extra.nodes);
    m.face_nodes.extend(&extra.face_nodes);
    m.triangles
        .extend(extra.triangles.iter().map(|t| t.map(|n| n + offset)));
    m.face_triangles.extend(&extra.face_triangles);
    m.face_numbers
        .extend(extra.face_numbers.iter().map(|n| n + faces));
    m.face_deflections.extend(&extra.face_deflections);
    m.face_tolerances.extend(&extra.face_tolerances);
    m.face_shells
        .extend(extra.face_shells.iter().map(|_| shell));
    m.edge_faces
        .extend(extra.edge_faces.iter().map(|f| f + faces));
    m.edge_numbers
        .extend(extra.edge_numbers.iter().map(|n| n + edges));
    m.edge_degenerate.extend(&extra.edge_degenerate);
    m.edge_vertices
        .extend(extra.edge_vertices.iter().map(|v| v.map(|v| v + vertices)));
    m.edge_deflections.extend(&extra.edge_deflections);
    m.edge_node_counts.extend(&extra.edge_node_counts);
    m.edge_nodes
        .extend(extra.edge_nodes.iter().map(|n| n + offset));
    m.validate().unwrap();
    m
}

/// `m` refused by `printable`: an error of `code` whose message says `says`, naming exactly
/// `faces`, with a suggestion.
#[track_caller]
pub fn refused(m: &ExportMesh, code: ErrorCode, says: &str, faces: &[&str]) {
    let error = printable(m, None, None).unwrap_err();
    assert_eq!(error.code(), code, "{error}");
    assert!(error.message().contains(says), "{error}");
    assert_eq!(error.elements(), faces, "{error}");
    assert_eq!(error.elements_total(), faces.len() as u64, "{error}");
    assert!(error.suggestion().is_some(), "{error}");
}
