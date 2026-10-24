//! break-it: M4.8's export core (silico_core::export) on hostile reports built by hand or
//! generated: reports that fail `ExportMesh::validate` (printable is public, and takes any
//! report), reports that pass it but describe no surface, cavities larger than their solid,
//! random corruptions of a good report checked against the promises of a printable mesh
//! (re-derived here, not read from the code under test), and archives of nothing. The real
//! kernel's meshes are silico-kernel's tests/adversarial_export.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::ErrorCode;
use silico_core::export::{Object, PrintableMesh, printable, three_mf};
use silico_protocol::ExportMesh;

/// The unit tetrahedron's corners: P0 at the origin, P1, P2, P3 on the axes.
const P: [[f64; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
];

/// Its faces, each one triangle counterclockwise seen from outside.
const TETRA: [&[[usize; 3]]; 4] = [&[[0, 2, 1]], &[[0, 1, 3]], &[[0, 3, 2]], &[[1, 2, 3]]];

/// A one-solid, one-shell report of faces made of `triangles` over `points`, as the worker
/// reports a B-rep's (the same construction as tests/export.rs): each face its own copy of
/// the points it uses, an edge use for each side of its triangles, numbered by the two
/// points it joins, its vertices the points'.
fn report(points: &[[f64; 3]], faces: &[&[[usize; 3]]]) -> ExportMesh {
    let mut m = ExportMesh {
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
        for (a, b) in sides {
            let next = edge_numbers.len() as u32 + 1;
            let number = *edge_numbers.entry((a, b)).or_insert(next);
            m.edge_faces.push(f as u32);
            m.edge_numbers.push(number);
            m.edge_degenerate.push(false);
            m.edge_vertices.push([a as u32 + 1, b as u32 + 1]);
            m.edge_deflections.push(0.0);
            m.edge_node_counts.push(2);
            let (na, nb) = (node(a, &mut m), node(b, &mut m));
            m.edge_nodes.extend([na, nb]);
        }
    }
    m.validate().unwrap();
    m
}

fn tetra() -> ExportMesh {
    report(&P, &TETRA)
}

/// `extra`'s faces appended to `base` as shell `shell` of solid `solid` (a new shell, and a
/// new solid if `solid` is past `base`'s), its edges and vertices renumbered past `base`'s
/// so the two share none.
fn with_shell(base: &ExportMesh, extra: &ExportMesh, solid: u32, outer: bool) -> ExportMesh {
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

/// `f`'s result, or its panic's message.
fn caught<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|panic| {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default()
    })
}

/// A change made to a good report.
type Corrupt = Box<dyn Fn(&mut ExportMesh)>;

#[test]
fn printable_on_a_report_that_fails_validate_is_an_error_not_a_panic() {
    // BREAKS: printable is public and takes any ExportMesh (its fields are public, and
    // nothing says it must be validated first), but it indexes the report unchecked: a
    // report that fails ExportMesh::validate panics it (index out of bounds) instead of
    // being refused. Only KernelRequest::export_mesh validates before handing one over.
    let corruptions: Vec<(&str, Corrupt)> = vec![
        ("faces in no shell", Box::new(|m| m.face_shells.clear())),
        ("a face in shell 5 of 1", Box::new(|m| m.face_shells[0] = 5)),
        (
            "a shell in solid 3 of 1",
            Box::new(|m| m.shell_solids[0] = 3),
        ),
        ("no outer flags", Box::new(|m| m.shell_outer.clear())),
        ("no face numbers", Box::new(|m| m.face_numbers.clear())),
        (
            "no face deflections",
            Box::new(|m| m.face_deflections.clear()),
        ),
        (
            "a triangle past the nodes",
            Box::new(|m| m.triangles[0][0] = 999),
        ),
        (
            "a face counting nodes it hasn't",
            Box::new(|m| m.face_nodes[0] += 100),
        ),
        (
            "triangles uncounted",
            Box::new(|m| m.face_triangles[0] += 5),
        ),
        ("an edge use on face 99", Box::new(|m| m.edge_faces[0] = 99)),
        (
            "an edge use of 50 nodes",
            Box::new(|m| m.edge_node_counts[0] = 50),
        ),
        (
            "an edge node past the nodes",
            Box::new(|m| m.edge_nodes[0] = 999),
        ),
        ("no edge vertices", Box::new(|m| m.edge_vertices.clear())),
        (
            "no degenerate flags",
            Box::new(|m| m.edge_degenerate.clear()),
        ),
        ("no edge numbers", Box::new(|m| m.edge_numbers.clear())),
        (
            "no edge deflections",
            Box::new(|m| m.edge_deflections.clear()),
        ),
    ];
    let mut panicked = Vec::new();
    for (what, corrupt) in &corruptions {
        let mut m = tetra();
        corrupt(&mut m);
        assert!(m.validate().is_err(), "{what}: validate took it");
        match caught(|| printable(&m, 0.1, None)) {
            Err(panic) => panicked.push(format!("{what}: {panic}")),
            Ok(Ok(_)) => panicked.push(format!("{what}: printable took it")),
            // Any refusal will do, rather than a panic.
            Ok(Err(_)) => {}
        }
    }
    assert!(panicked.is_empty(), "{panicked:#?}");
}

#[test]
fn a_solid_whose_shell_has_no_faces_is_refused_not_an_empty_mesh() {
    // BREAKS: a report of one solid whose one shell has no faces passes validate, and
    // printable returns it as a printable mesh of no vertices, no triangles, and volume 0:
    // rule 5 is checked triangle by triangle, so a shell with none is never checked. The
    // port note (docs/ports/mesh-export.md, da0f810282) says "A solid with no triangles is
    // refused".
    let empty = ExportMesh {
        deflection: 0.1,
        solids: 1,
        shell_solids: vec![0],
        shell_outer: vec![true],
        ..ExportMesh::default()
    };
    empty.validate().unwrap();
    let got = printable(&empty, 0.1, None);
    assert!(
        got.is_err(),
        "an empty solid exported as {:?}",
        got.map(|m| (m.vertices().len(), m.triangles().len(), m.volume()))
    );
    // Beside a good solid, the empty one is dropped without a word.
    let mut two = tetra();
    two.solids = 2;
    two.shell_solids.push(1);
    two.shell_outer.push(true);
    two.validate().unwrap();
    let got = printable(&two, 0.1, None);
    assert!(
        got.is_err(),
        "a solid of no faces beside a tetrahedron exported as {:?}",
        got.map(|m| (m.triangles().len(), m.volume()))
    );
}

#[test]
fn a_face_whose_triangles_all_weld_into_one_vertex_is_refused_not_an_empty_mesh() {
    // BREAKS: one face of three nodes at one point, one triangle over them, and a degenerate
    // edge whose polygon holds all three (as a pole's does): the nodes weld into one
    // vertex, the triangle is dropped as a pole's, and what's left is an empty "printable"
    // mesh of volume 0, which the port note says is refused.
    let m = ExportMesh {
        deflection: 0.1,
        nodes: vec![[1.0, 2.0, 3.0]; 3],
        face_nodes: vec![3],
        triangles: vec![[0, 1, 2]],
        face_triangles: vec![1],
        face_numbers: vec![1],
        face_deflections: vec![0.0],
        face_shells: vec![0],
        shell_solids: vec![0],
        shell_outer: vec![true],
        solids: 1,
        loose_faces: vec![],
        edge_faces: vec![0],
        edge_numbers: vec![1],
        edge_degenerate: vec![true],
        edge_vertices: vec![[1, 1]],
        edge_deflections: vec![0.0],
        edge_node_counts: vec![3],
        edge_nodes: vec![0, 1, 2],
    };
    m.validate().unwrap();
    let got = printable(&m, 0.1, None);
    assert!(
        got.is_err(),
        "a face welded to a point exported as {:?}",
        got.map(|m| (m.vertices().len(), m.triangles().len(), m.volume()))
    );
}

#[test]
fn a_cavity_larger_than_its_solid_is_refused_not_a_negative_volume() {
    // BREAKS: rule 5 checks each shell's sign on its own, and the port note adds "The
    // solid's total is positive", but nothing checks the total: an outer shell of 1/6 with
    // a cavity shell (correctly inward) of 8/6 around it is taken, and the printable mesh's
    // volume is -7/6.
    let big: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| 2.0 * c - 0.5)).collect();
    let inward: Vec<[[usize; 3]; 1]> = TETRA
        .iter()
        .map(|f| [[f[0][0], f[0][2], f[0][1]]])
        .collect();
    let faces: Vec<&[[usize; 3]]> = inward.iter().map(|f| f.as_slice()).collect();
    let cavity = report(&big, &faces);
    let m = with_shell(&tetra(), &cavity, 0, false);
    let got = printable(&m, 0.1, None);
    assert!(
        got.is_err(),
        "exported with volume {:?}",
        got.map(|m| m.volume())
    );
}

#[test]
fn a_solid_whose_only_shell_is_a_cavity_is_refused() {
    // OCCT's BRepClass3d::OuterShell can find no outer shell (a null shell, which no shell
    // IsSame): every shell of the solid is then reported as a cavity's. An inward-facing
    // tetrahedron as the only shell, flagged a cavity, would pass rule 5 on its own.
    let inward: Vec<[[usize; 3]; 1]> = TETRA
        .iter()
        .map(|f| [[f[0][0], f[0][2], f[0][1]]])
        .collect();
    let faces: Vec<&[[usize; 3]]> = inward.iter().map(|f| f.as_slice()).collect();
    let mut m = report(&P, &faces);
    m.shell_outer = vec![false];
    // BREAKS: taken, with volume -1/6: a solid with no outer shell is exported inside out
    // (the existing test, a_shell_facing_inward_is_refused_and_a_cavity_must_face_inward,
    // asserts exactly this is Ok; the solid's total is never checked).
    let got = printable(&m, 0.1, None);
    assert!(
        got.is_err(),
        "exported with volume {:?}",
        got.map(|m| m.volume())
    );
}

#[test]
fn a_node_welded_through_a_chain_of_vertices_is_checked_against_where_it_lands() {
    // Each node is checked against its vertex's root only. A chain P1 -> P1' -> P1'' of
    // nodes each within the tolerance of the root is fine; this checks the root is where
    // the vertex is written, so every welded node is within the tolerance of the written
    // vertex.
    let mut m = tetra();
    let p1_face4 = m.face_ranges()[3].start;
    m.nodes[p1_face4 as usize] = [1.0 + 0.05, 0.0, 0.0];
    let mesh = printable(&m, 0.1, None).unwrap();
    assert!(mesh.vertices().contains(&[1.0, 0.0, 0.0]));
    assert!(!mesh.vertices().contains(&[1.05, 0.0, 0.0]));
}

#[test]
fn a_report_far_enough_out_that_32_bits_overflow_is_refused_without_panicking() {
    // Finite in 64 bits, past f32::MAX: the vertices written would be infinite.
    let far: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| c * 1e39 + 1e39)).collect();
    let m = report(&far, &TETRA);
    let error = printable(&m, 0.1, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
}

#[test]
fn printable_is_the_same_every_time() {
    let m = with_shell(&tetra(), &tetra(), 1, true);
    let first = printable(&m, 0.1, None).unwrap();
    for _ in 0..20 {
        assert_eq!(printable(&m, 0.1, None).unwrap(), first);
    }
}

/// The promises of a printable mesh, checked from its vertices and triangles alone: every
/// index in range and the three of a triangle distinct; every edge on exactly two
/// triangles, running opposite ways; no triangle of zero area in its 32-bit coordinates;
/// the volume the signed sum of its tetrahedra, and positive.
fn assert_printable(mesh: &PrintableMesh) -> Result<(), TestCaseError> {
    let n = mesh.vertices().len() as u32;
    let mut sides: BTreeMap<(u32, u32), Vec<bool>> = BTreeMap::new();
    let wide = |v: u32| mesh.vertices()[v as usize].map(f64::from);
    let mut volume = 0.0;
    prop_assert!(!mesh.triangles().is_empty(), "no triangles");
    for t in mesh.triangles() {
        prop_assert!(t.iter().all(|&v| v < n), "{t:?} past {n} vertices");
        prop_assert!(t[0] != t[1] && t[1] != t[2] && t[0] != t[2], "{t:?}");
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            sides.entry((a.min(b), a.max(b))).or_default().push(a < b);
        }
        let [a, b, c] = t.map(wide);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ];
        prop_assert!(cross != [0.0; 3], "{t:?} has no area");
        let bc = [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ];
        volume += (a[0] * bc[0] + a[1] * bc[1] + a[2] * bc[2]) / 6.0;
    }
    for (edge, ways) in &sides {
        prop_assert_eq!(ways.len(), 2, "edge {:?} on {} triangles", edge, ways.len());
        prop_assert!(ways[0] != ways[1], "edge {edge:?} runs one way in both");
    }
    prop_assert!(
        (volume - mesh.volume()).abs() <= 1e-9 * volume.abs().max(1.0),
        "volume {} against {volume}",
        mesh.volume()
    );
    prop_assert!(mesh.volume() > 0.0, "volume {}", mesh.volume());
    Ok(())
}

/// One corruption of a report, as a hostile or buggy worker might send it.
#[derive(Debug, Clone)]
enum Corruption {
    FlipOuter(usize),
    SwapCorners(usize),
    EdgeNumber(usize, u32),
    EdgeVertex(usize, usize, u32),
    Degenerate(usize),
    Coordinate(usize, usize, f64),
    FaceDeflection(usize, f64),
    EdgeDeflection(usize, f64),
    FaceShell(usize, u32),
    ReverseEdgeNodes(usize),
    DropFace(usize),
    Duplicate(usize),
}

fn corruption() -> impl Strategy<Value = Corruption> {
    let edge_value = prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(1.0),
        Just(0.5),
        Just(1.0 + 1e-9),
        Just(1e-9),
        Just(-1.0),
        Just(1e30),
        Just(f64::from(f32::MAX) * 2.0),
        Just(f64::MIN_POSITIVE),
        Just(1e6),
        -2.0..2.0f64,
    ];
    let deflection = prop_oneof![
        Just(0.0),
        Just(0.1),
        Just(0.1 - 1e-7),
        Just(0.1 - 2e-7),
        Just(1e300),
        Just(f64::MIN_POSITIVE),
    ];
    prop_oneof![
        (0..4usize).prop_map(Corruption::FlipOuter),
        (0..16usize).prop_map(Corruption::SwapCorners),
        (0..64usize, 1..12u32).prop_map(|(k, n)| Corruption::EdgeNumber(k, n)),
        (0..64usize, 0..2usize, 1..10u32).prop_map(|(k, e, v)| Corruption::EdgeVertex(k, e, v)),
        (0..64usize).prop_map(Corruption::Degenerate),
        (0..64usize, 0..3usize, edge_value).prop_map(|(n, a, c)| Corruption::Coordinate(n, a, c)),
        (0..16usize, deflection.clone()).prop_map(|(f, d)| Corruption::FaceDeflection(f, d)),
        (0..64usize, deflection).prop_map(|(k, d)| Corruption::EdgeDeflection(k, d)),
        (0..16usize, 0..2u32).prop_map(|(f, s)| Corruption::FaceShell(f, s)),
        (0..64usize).prop_map(Corruption::ReverseEdgeNodes),
        (0..16usize).prop_map(Corruption::DropFace),
        (0..16usize).prop_map(Corruption::Duplicate),
    ]
}

/// `m` with `c` applied, where it applies (indices taken modulo what there is).
fn corrupt(m: &mut ExportMesh, c: &Corruption) {
    let faces = m.face_nodes.len();
    let uses = m.edge_faces.len();
    match *c {
        Corruption::FlipOuter(s) => {
            let s = s % m.shell_outer.len();
            m.shell_outer[s] = !m.shell_outer[s];
        }
        Corruption::SwapCorners(t) if !m.triangles.is_empty() => {
            let t = t % m.triangles.len();
            m.triangles[t].swap(1, 2);
        }
        Corruption::EdgeNumber(k, n) if uses > 0 => m.edge_numbers[k % uses] = n,
        Corruption::EdgeVertex(k, e, v) if uses > 0 => m.edge_vertices[k % uses][e] = v,
        Corruption::Degenerate(k) if uses > 0 => {
            m.edge_degenerate[k % uses] = !m.edge_degenerate[k % uses];
        }
        Corruption::Coordinate(n, a, c) if !m.nodes.is_empty() => {
            let n = n % m.nodes.len();
            m.nodes[n][a] = c;
        }
        Corruption::FaceDeflection(f, d) if faces > 0 => m.face_deflections[f % faces] = d,
        Corruption::EdgeDeflection(k, d) if uses > 0 => m.edge_deflections[k % uses] = d,
        Corruption::FaceShell(f, s) if faces > 0 => {
            m.face_shells[f % faces] = s % m.shell_solids.len() as u32;
        }
        Corruption::ReverseEdgeNodes(k) if uses > 0 => {
            let k = k % uses;
            let start: u32 = m.edge_node_counts[..k].iter().sum();
            let end = start + m.edge_node_counts[k];
            m.edge_nodes[start as usize..end as usize].reverse();
        }
        Corruption::DropFace(f) if faces > 0 => {
            // The face's triangles and polygons gone, as an unmeshed face's: no nodes either.
            let f = f % faces;
            let t_start: u32 = m.face_triangles[..f].iter().sum();
            let t_end = t_start + m.face_triangles[f];
            m.triangles.drain(t_start as usize..t_end as usize);
            m.face_triangles[f] = 0;
            for k in 0..uses {
                if m.edge_faces[k] as usize == f {
                    let start: u32 = m.edge_node_counts[..k].iter().sum();
                    let end = start + m.edge_node_counts[k];
                    m.edge_nodes.drain(start as usize..end as usize);
                    m.edge_node_counts[k] = 0;
                }
            }
        }
        Corruption::Duplicate(t) if !m.triangles.is_empty() => {
            // A triangle repeated in its own face.
            let t = t % m.triangles.len();
            let mut face = 0;
            let mut seen = 0u32;
            while seen + m.face_triangles[face] <= t as u32 {
                seen += m.face_triangles[face];
                face += 1;
            }
            let copy = m.triangles[t];
            m.triangles.insert(t, copy);
            m.face_triangles[face] += 1;
        }
        _ => {}
    }
}

#[test]
fn corrupted_reports_never_panic_and_whatever_is_taken_is_printable() {
    // Start from a good report (a tetrahedron, or one with a second solid, or a cavity),
    // corrupt it up to four ways, and keep those validate takes: printable must not panic,
    // and a mesh it returns must keep every promise of a printable mesh, checked here from
    // the mesh alone.
    let small: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| 0.5 * c + 0.2)).collect();
    let inward: Vec<[[usize; 3]; 1]> = TETRA
        .iter()
        .map(|f| [[f[0][0], f[0][2], f[0][1]]])
        .collect();
    let faces: Vec<&[[usize; 3]]> = inward.iter().map(|f| f.as_slice()).collect();
    let bases = [
        tetra(),
        with_shell(&tetra(), &tetra(), 1, true),
        with_shell(&tetra(), &report(&small, &faces), 0, false),
    ];
    let mut runner = TestRunner::new(Config {
        cases: 4000,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(
            &(0..bases.len(), prop::collection::vec(corruption(), 1..5)),
            |(base, corruptions)| {
                let mut m = bases[base].clone();
                for c in &corruptions {
                    corrupt(&mut m, c);
                }
                if m.validate().is_err() {
                    return Ok(());
                }
                let got = caught(|| printable(&m, 0.1, None));
                let got = match got {
                    Err(panic) => {
                        return Err(TestCaseError::fail(format!("panicked: {panic}")));
                    }
                    Ok(got) => got,
                };
                match got {
                    Ok(mesh) => assert_printable(&mesh)?,
                    Err(error) => {
                        prop_assert!(
                            matches!(
                                error.code(),
                                ErrorCode::InvalidResult | ErrorCode::InvalidInput
                            ),
                            "{error}"
                        );
                        prop_assert!(error.suggestion().is_some(), "{error}");
                        prop_assert!(!error.elements().is_empty(), "{error}");
                    }
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn an_archive_of_no_objects_is_refused() {
    // BREAKS: three_mf(&[]) writes a 3MF with no object and an empty build, which a slicer
    // opens as nothing. FreeCAD refuses an export with nothing to mesh ("None of the
    // objects can be exported", 04005b5e45), and the port note ports that guard.
    let got = three_mf(&[]);
    assert!(
        got.is_err(),
        "wrote {} bytes",
        got.map(|b| b.len()).unwrap_or(0)
    );
}

#[test]
fn a_part_number_xml_cant_hold_is_refused_naming_the_object_and_saying_which() {
    let mesh = printable(&tetra(), 0.1, None).unwrap();
    let error = three_mf(&[Object {
        name: "Bracket",
        part_number: "k7\u{0}x",
        mesh: &mesh,
    }])
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.message().contains("part number"), "{error}");
    assert_eq!(error.elements(), ["Bracket"]);
    // A later object's bad name, after a good one: refused, nothing written.
    let error = three_mf(&[
        Object {
            name: "Good",
            part_number: "a",
            mesh: &mesh,
        },
        Object {
            name: "Bad\u{7}",
            part_number: "b",
            mesh: &mesh,
        },
    ])
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
}

#[test]
fn every_character_xml_1_0_holds_is_written_and_every_other_refused() {
    // XML 1.0's Char: #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] |
    // [#x10000-#x10FFFF]. Every other scalar value (C0 controls, U+FFFE, U+FFFF) refused.
    let mesh = printable(&tetra(), 0.1, None).unwrap();
    let write = |name: &str| {
        three_mf(&[Object {
            name,
            part_number: "p",
            mesh: &mesh,
        }])
    };
    for c in (0u32..0x11_0000)
        .filter_map(char::from_u32)
        .step_by(97)
        .chain([
            '\u{0}',
            '\u{8}',
            '\u{9}',
            '\u{A}',
            '\u{B}',
            '\u{C}',
            '\u{D}',
            '\u{1F}',
            '\u{20}',
            '\u{7F}',
            '\u{85}',
            '\u{D7FF}',
            '\u{E000}',
            '\u{FFFD}',
            '\u{FFFE}',
            '\u{FFFF}',
            '\u{10000}',
            '\u{10FFFF}',
        ])
    {
        let code = c as u32;
        let holds = matches!(code, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x1_0000..);
        let got = write(&format!("a{c}b"));
        assert_eq!(got.is_ok(), holds, "U+{code:04X}");
    }
}

#[test]
fn an_archives_size_is_what_its_limit_counts() {
    // The 4 GiB check counts 76 bytes and the name twice per entry, and 22 for the end: the
    // archive written must be exactly that long, or the limit is off.
    let mesh = printable(&tetra(), 0.1, None).unwrap();
    let archive = three_mf(&[Object {
        name: "T",
        part_number: "p",
        mesh: &mesh,
    }])
    .unwrap();
    // Each local header's sizes, read back, give the entries' data lengths.
    let u16_at = |at: usize| usize::from(u16::from_le_bytes([archive[at], archive[at + 1]]));
    let u32_at = |at: usize| u32::from_le_bytes(archive[at..at + 4].try_into().unwrap()) as usize;
    let mut at = 0;
    let mut expected = 22;
    while u32_at(at) == 0x0403_4b50 {
        let (size, name) = (u32_at(at + 22), u16_at(at + 26));
        expected += 76 + 2 * name + size;
        at += 30 + name + size;
    }
    assert_eq!(archive.len(), expected);
}
