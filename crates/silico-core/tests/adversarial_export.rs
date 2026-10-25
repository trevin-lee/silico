//! break-it: M4.8's export core (silico_core::export) on hostile reports built by hand or
//! generated: reports that fail `ExportMesh::validate` (printable is public, and takes any
//! report), reports that pass it but describe no surface, cavities larger than their solid,
//! random corruptions of a good report checked against the promises of a printable mesh
//! (re-derived here, not read from the code under test), and archives of nothing. The real
//! kernel's meshes are silico-kernel's tests/adversarial_export.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;

use super::export_report::{P, TETRA, refused, report, tetra, with_shell};

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::ErrorCode;
use silico_core::export::{Object, PrintableMesh, needs_unraised, printable, three_mf};
use silico_protocol::ExportMesh;

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
    // Break-it found (fixed): printable is public and takes any ExportMesh (its fields are
    // public, and nothing says it must be validated first), but it indexes the report
    // unchecked: a report that fails ExportMesh::validate panics it (index out of bounds)
    // instead of being refused. Only KernelRequest::export_mesh validates before handing
    // one over.
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
        match caught(|| printable(&m, None, None)) {
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
    // Break-it found (fixed): a report of one solid whose one shell has no faces passes
    // validate, and printable returns it as a printable mesh of no vertices, no triangles,
    // and volume 0: rule 5 is checked triangle by triangle, so a shell with none is never
    // checked. The port note (docs/ports/mesh-export.md, da0f810282) says "A solid with no
    // triangles is refused".
    let empty = ExportMesh {
        tolerance: 0.1,
        deflection: 0.1,
        solids: 1,
        shell_solids: vec![0],
        shell_outer: vec![true],
        ..ExportMesh::default()
    };
    empty.validate().unwrap();
    refused(
        &empty,
        ErrorCode::InvalidResult,
        "a shell of the solid encloses no volume",
        &[],
    );
    // Beside a good solid, the empty one is dropped without a word.
    let mut two = tetra();
    two.solids = 2;
    two.shell_solids.push(1);
    two.shell_outer.push(true);
    two.validate().unwrap();
    refused(
        &two,
        ErrorCode::InvalidResult,
        "a shell of the solid encloses no volume",
        &[],
    );
}

#[test]
fn a_face_whose_triangles_all_weld_into_one_vertex_is_refused_not_an_empty_mesh() {
    // Break-it found (fixed): one face of three nodes at one point, one triangle over them,
    // and a degenerate edge whose polygon holds all three (as a pole's does): the nodes
    // weld into one vertex, the triangle is dropped as a pole's, and what's left is an
    // empty "printable" mesh of volume 0, which the port note says is refused.
    let m = ExportMesh {
        tolerance: 0.1,
        deflection: 0.1,
        force_face_deflection: false,
        nodes: vec![[1.0, 2.0, 3.0]; 3],
        face_nodes: vec![3],
        triangles: vec![[0, 1, 2]],
        face_triangles: vec![1],
        face_numbers: vec![1],
        face_deflections: vec![0.0],
        face_shells: vec![0],
        face_tolerances: vec![1e-7],
        unoriented_faces: vec![],
        unoriented_shells: vec![],
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
    refused(
        &m,
        ErrorCode::InvalidResult,
        "a shell of the solid encloses no volume",
        &["Face1"],
    );
}

#[test]
fn a_cavity_larger_than_its_solid_is_refused_not_a_negative_volume() {
    // Break-it found (fixed): rule 5 checks each shell's sign on its own, and the port note
    // adds "The solid's total is positive", but nothing checks the total: an outer shell of
    // 1/6 with a cavity shell (correctly inward) of 8/6 around it is taken, and the
    // printable mesh's volume is -7/6.
    let big: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| 2.0 * c - 0.5)).collect();
    let inward: Vec<[[usize; 3]; 1]> = TETRA
        .iter()
        .map(|f| [[f[0][0], f[0][2], f[0][1]]])
        .collect();
    let faces: Vec<&[[usize; 3]]> = inward.iter().map(|f| f.as_slice()).collect();
    let cavity = report(&big, &faces);
    let m = with_shell(&tetra(), &cavity, 0, false);
    let all: Vec<String> = (1..=8).map(|f| format!("Face{f}")).collect();
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    refused(
        &m,
        ErrorCode::InvalidResult,
        "encloses no volume in all",
        &all,
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
    // Break-it found (fixed): taken, with volume -1/6: a solid with no outer shell is
    // exported inside out (the existing test,
    // a_shell_facing_inward_is_refused_and_a_cavity_must_face_inward, asserts exactly this
    // is Ok; the solid's total is never checked).
    refused(
        &m,
        ErrorCode::InvalidResult,
        "encloses no volume in all",
        &["Face1", "Face2", "Face3", "Face4"],
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
    let mesh = printable(&m, None, None).unwrap();
    assert!(mesh.vertices().contains(&[1.0, 0.0, 0.0]));
    assert!(!mesh.vertices().contains(&[1.05, 0.0, 0.0]));
}

#[test]
fn a_report_far_enough_out_that_32_bits_overflow_is_refused_without_panicking() {
    // Finite in 64 bits, past f32::MAX: the vertices written would be infinite.
    let far: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| c * 1e39 + 1e39)).collect();
    let m = report(&far, &TETRA);
    // Refused as the worker refuses such a part: the rounding to 32 bits takes more than
    // half the tolerance (all of it, past f32::MAX), the tolerance named.
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
}

#[test]
fn printable_is_the_same_every_time() {
    let m = with_shell(&tetra(), &tetra(), 1, true);
    let first = printable(&m, None, None).unwrap();
    for _ in 0..20 {
        assert_eq!(printable(&m, None, None).unwrap(), first);
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
                let got = caught(|| printable(&m, None, None));
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
                        // A refusal names the faces at fault, or the tolerance where the
                        // part is too far out for it (the whole part's, no face's).
                        prop_assert!(
                            !error.elements().is_empty() || error.parameter() == Some("tolerance"),
                            "{error}"
                        );
                    }
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn an_archive_of_no_objects_is_refused() {
    // Break-it found (fixed): three_mf(&[]) writes a 3MF with no object and an empty build,
    // which a slicer opens as nothing. FreeCAD refuses an export with nothing to mesh
    // ("None of the objects can be exported", 04005b5e45), and the port note ports that
    // guard.
    let error = three_mf(&[]).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.message().contains("at least one object"), "{error}");
    assert!(error.suggestion().is_some(), "{error}");
}

#[test]
fn a_part_number_xml_cant_hold_is_refused_naming_the_object_and_saying_which() {
    let mesh = printable(&tetra(), None, None).unwrap();
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
    let mesh = printable(&tetra(), None, None).unwrap();
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
    let mesh = printable(&tetra(), None, None).unwrap();
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

#[test]
fn a_report_claiming_more_solids_than_shells_is_refused_without_allocating_for_them() {
    // The council's robustness lens: a count of solids no shell bears out made printable
    // allocate for each (32 GiB for u32::MAX). Every solid has a shell, so validate refuses
    // more solids than shells, before anything is sized by the count.
    let mut m = tetra();
    m.solids = u32::MAX;
    assert!(m.validate().is_err());
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.message().contains("doesn't hold together"), "{error}");
    assert!(
        error.cause().is_some_and(|c| c.contains("solids")),
        "{error}"
    );
}

#[test]
fn a_vertex_past_32_bits_is_refused_even_with_no_edge_to_check() {
    // The council's robustness lens: one face holding all four of the tetrahedron's
    // triangles, P1 at 1e39 (past f32::MAX, so infinite as written), and no edge uses: the
    // volume came out NaN, which no volume check refused, and the 3MF was written with
    // x="inf". The rounding rule, applied to the vertices, refuses it, naming the tolerance.
    let mut points = P;
    points[1] = [1e39, 0.0, 0.0];
    let one_face: [[usize; 3]; 4] = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]];
    let mut m = report(&points, &[&one_face]);
    m.edge_faces.clear();
    m.edge_numbers.clear();
    m.edge_degenerate.clear();
    m.edge_vertices.clear();
    m.edge_deflections.clear();
    m.edge_node_counts.clear();
    m.edge_nodes.clear();
    m.validate().unwrap();
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
    // And at 1e30, finite in 32 bits but past what the rounding allows, likewise.
    let mut points = P;
    points[1] = [1e30, 0.0, 0.0];
    let mut m = report(&points, &[&one_face]);
    m.edge_faces.clear();
    m.edge_numbers.clear();
    m.edge_degenerate.clear();
    m.edge_vertices.clear();
    m.edge_deflections.clear();
    m.edge_node_counts.clear();
    m.edge_nodes.clear();
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
}

#[test]
fn a_report_with_a_value_that_isnt_finite_or_an_element_numbered_0_is_refused() {
    // The council's tests lens: validate's finiteness and numbering checks are all that
    // keep NaN geometry from being exported.
    let corruptions: [fn(&mut ExportMesh); 12] = [
        |m| m.nodes[0][0] = f64::NAN,
        |m| m.nodes[5][2] = f64::INFINITY,
        |m| m.edge_deflections[0] = f64::NAN,
        |m| m.edge_deflections[0] = -1.0,
        |m| m.deflection = f64::NAN,
        |m| m.face_numbers[0] = 0,
        |m| m.edge_vertices[0][1] = 0,
        // A face's tolerance, one per face, a finite length.
        |m| m.face_tolerances[2] = f64::NAN,
        |m| {
            m.face_tolerances.pop();
        },
        // An unoriented face numbered from 1, and neither a face of the mesh nor loose.
        |m| m.unoriented_faces = vec![0],
        |m| m.unoriented_faces = vec![2],
        // A deflection past the tolerance it was meant to meet.
        |m| m.deflection = 0.2,
    ];
    for (k, corrupt) in corruptions.iter().enumerate() {
        let mut m = tetra();
        corrupt(&mut m);
        assert!(m.validate().is_err(), "corruption {k}");
        let error = printable(&m, None, None).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{k}: {error}");
        assert!(
            error.message().contains("doesn't hold together"),
            "{k}: {error}"
        );
    }
}

#[test]
fn a_refusal_names_the_first_hundred_faces_and_counts_them_all() {
    let mut m = tetra();
    m.loose_faces = (1..=150).collect();
    let error = printable(&m, None, None).unwrap_err();
    let first: Vec<String> = (1..=100).map(|f| format!("Face{f}")).collect();
    assert_eq!(error.elements(), first, "{error}");
    assert_eq!(error.elements_total(), 150, "{error}");
}

#[test]
fn a_faces_node_at_minus_0_is_one_with_its_node_at_0() {
    // The mutants run on #123 found it untested: a face's nodes at one place are joined by
    // their coordinates' bits, with -0 taken as 0. Face4's P1 is given a second node at
    // (1, -0, -0), and Face4's triangle uses it while its edges use the first: joined, the
    // mesh is the tetrahedron; apart, the triangle's sides meet no other face's.
    let mut m = tetra();
    let face4 = m.face_ranges()[3].clone();
    let p1 = face4
        .clone()
        .find(|&n| m.nodes[n as usize] == P[1])
        .unwrap();
    m.nodes.push([1.0, -0.0, -0.0]);
    m.face_nodes[3] += 1;
    let twin = m.nodes.len() as u32 - 1;
    let last = m.triangles.len() - 1;
    for n in &mut m.triangles[last] {
        if *n == p1 {
            *n = twin;
        }
    }
    m.validate().unwrap();
    let mesh = printable(&m, None, None).unwrap();
    assert_eq!(mesh.vertices().len(), 4);
    assert_eq!(mesh.triangles().len(), 4);
    assert_eq!(mesh.volume(), 1.0 / 6.0);
}

#[test]
fn a_poles_nodes_are_one_vertex_though_their_coordinates_differ() {
    // The mutants run on #123 found it untested: a degenerate edge's nodes are joined at its
    // vertex (an edge's ends are also joined through the edges they pair on, so only a
    // pole's inner nodes rest on it). Face4's apex, P3, is split into three nodes a
    // nanometre apart, as OCCT leaves a pole's, with a degenerate edge through them, and
    // Face4 fanned from P1 over them: P1 P2 P3, P1 P3 P3', P1 P3' P3''. Joined, the fan's
    // last two triangles have no area and are left out, and the mesh is the tetrahedron;
    // with P3' apart, its two triangles don't close.
    let mut m = tetra();
    let face4 = m.face_ranges()[3].clone();
    let at =
        |m: &ExportMesh, p: [f64; 3]| face4.clone().find(|&n| m.nodes[n as usize] == p).unwrap();
    let (p1, p3) = (at(&m, P[1]), at(&m, P[3]));
    let middle = m.nodes.len() as u32;
    let end = middle + 1;
    m.nodes.extend([[1e-6, 0.0, 1.0], [2e-6, 0.0, 1.0]]);
    m.face_nodes[3] += 2;
    m.triangles.extend([[p1, p3, middle], [p1, middle, end]]);
    m.face_triangles[3] += 2;
    // Face4's edge from P1 to P3 now ends at the fan's last node.
    let k = (0..m.edge_faces.len())
        .find(|&k| m.edge_faces[k] == 3 && m.edge_vertices[k] == [2, 4])
        .unwrap();
    let start: u32 = m.edge_node_counts[..k].iter().sum();
    assert_eq!(m.edge_nodes[start as usize + 1], p3);
    m.edge_nodes[start as usize + 1] = end;
    m.edge_faces.push(3);
    m.edge_numbers.push(7);
    m.edge_degenerate.push(true);
    m.edge_vertices.push([4, 4]);
    m.edge_deflections.push(0.0);
    m.edge_node_counts.push(3);
    m.edge_nodes.extend([p3, middle, end]);
    m.validate().unwrap();
    let mesh = printable(&m, None, None).unwrap();
    assert_eq!(mesh.vertices().len(), 4);
    assert_eq!(mesh.triangles().len(), 4);
    assert!(mesh.vertices().contains(&[0.0, 0.0, 1.0]));
}

#[test]
fn a_joined_node_the_allowed_distance_away_is_taken_and_one_further_refused() {
    // The mutants run on #123 found the bound untested at its edge: within the tolerance,
    // less what rounding to 32 bits can move a vertex as far out as the part reaches (1 mm
    // here), is within. Face2's P0 is moved along x by exactly that (a square's square root
    // is the length exactly), then by the next length up.
    let allowed = silico_protocol::export_deflection(0.1, 1.0).unwrap();
    for (offset, taken) in [(allowed, true), (allowed.next_up(), false)] {
        let mut m = tetra();
        let p0 = m.face_ranges()[1]
            .clone()
            .find(|&n| m.nodes[n as usize] == P[0])
            .unwrap();
        m.nodes[p0 as usize] = [offset, 0.0, 0.0];
        if taken {
            printable(&m, None, None).unwrap();
        } else {
            refused(
                &m,
                ErrorCode::InvalidResult,
                "further than the tolerance",
                &["Face1", "Face2"],
            );
        }
    }
}

#[test]
fn a_solid_with_no_shell_is_refused() {
    // The mutants run on #123 found it untested: a solid the worker counts with no shell
    // (as an empty TopoDS_Solid is), beside one of two shells so the count of solids passes
    // validate, encloses nothing and is refused, with no face to name.
    let apart: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| c + 5.0)).collect();
    let mut m = with_shell(&tetra(), &report(&apart, &TETRA), 0, true);
    m.solids = 2;
    m.validate().unwrap();
    refused(
        &m,
        ErrorCode::InvalidResult,
        "encloses no volume in all",
        &[],
    );
}

/// The tetrahedron with the faces at `loose` (by index) `tolerance` mm loose, and OCCT's
/// unraised mesh of it, the same nodes.
fn loose_tetra(loose: &[(usize, f64)]) -> (ExportMesh, ExportMesh) {
    let mut m = tetra();
    for &(face, tolerance) in loose {
        m.face_tolerances[face] = tolerance;
    }
    let mut unraised = m.clone();
    unraised.force_face_deflection = true;
    (m, unraised)
}

#[test]
fn a_face_raised_by_any_tolerance_past_half_the_deflection_needs_the_unraised_mesh() {
    // OCCT raises a face's deflection where twice its tolerance is past it
    // (BRepMesh_Deflection.cxx:149-152): at exactly half it's not raised.
    let (m, _) = loose_tetra(&[(3, 0.05)]);
    assert!(!needs_unraised(&m));
    printable(&m, None, None).unwrap();
    let (m, _) = loose_tetra(&[(3, 0.05f64.next_up())]);
    assert!(needs_unraised(&m));
    assert!(!needs_unraised(&tetra()));
}

#[test]
fn a_raised_face_with_the_nodes_it_has_unraised_is_exported_in_any_order() {
    let (m, mut unraised) = loose_tetra(&[(3, 0.06)]);
    let face4 = unraised.face_ranges()[3].clone();
    unraised.nodes[face4.start as usize..face4.end as usize].reverse();
    let mesh = printable(&m, Some(&unraised), None).unwrap();
    assert_eq!(mesh.vertices().len(), 4);
    assert_eq!(mesh.volume(), 1.0 / 6.0);
}

#[test]
fn a_raised_face_whose_nodes_move_unraised_is_refused_alone_with_the_tolerance_to_export_at() {
    // Face2 and Face4 raised, only Face4's nodes moved (one coordinate an ulp): Face4 is
    // named alone, at twice its tolerance, 0.06 mm.
    let (m, mut unraised) = loose_tetra(&[(1, 0.06), (3, 0.06)]);
    let face4 = unraised.face_ranges()[3].start as usize;
    unraised.nodes[face4][0] = unraised.nodes[face4][0].next_up();
    let error = printable(&m, Some(&unraised), None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
    assert_eq!(error.elements(), ["Face4"], "{error}");
    assert!(
        error.message().contains(
            "at 0.12 mm, not the 0.1 mm asked, and placed other nodes than it would at 0.1"
        ) && error.message().contains("only exact to 0.06 mm"),
        "{error}"
    );
    // Meshed at the tolerance itself (no rounding margin), 0.12 mm is where 2 × 0.06 stops
    // being past the deflection; on the nanometre exactly, the suggestion is the next one,
    // as the margin the worker meshed with is known only to an ulp of the tolerance.
    assert_eq!(
        error.suggestion(),
        Some(
            "export with a tolerance of at least 0.120001 mm, or rebuild the part so the \
             faces named meet more precisely"
        ),
        "{error}"
    );
    // Both moved, both named, and the looser's tolerance given: 2 × 0.07.
    let (m, mut unraised) = loose_tetra(&[(1, 0.06), (3, 0.07)]);
    for face in [1, 3] {
        let first = unraised.face_ranges()[face].start as usize;
        unraised.nodes[first][1] += 1e-3;
    }
    let error = printable(&m, Some(&unraised), None).unwrap_err();
    assert_eq!(error.elements(), ["Face2", "Face4"], "{error}");
    assert!(error.message().contains("at 0.14 mm"), "{error}");
    assert!(
        error.suggestion().unwrap().contains("at least 0.140001 mm"),
        "{error}"
    );
}

#[test]
fn a_raised_face_unmeshed_unraised_is_refused() {
    // Face4, the last, with no nodes, triangles, or edge nodes when unraised.
    let (m, mut unraised) = loose_tetra(&[(3, 0.06)]);
    let nodes = unraised.face_nodes[3] as usize;
    let triangles = unraised.face_triangles[3] as usize;
    unraised.nodes.truncate(unraised.nodes.len() - nodes);
    unraised
        .triangles
        .truncate(unraised.triangles.len() - triangles);
    unraised.face_nodes[3] = 0;
    unraised.face_triangles[3] = 0;
    let mut dropped = 0;
    for k in 0..unraised.edge_faces.len() {
        if unraised.edge_faces[k] == 3 {
            dropped += unraised.edge_node_counts[k] as usize;
            unraised.edge_node_counts[k] = 0;
        }
    }
    unraised
        .edge_nodes
        .truncate(unraised.edge_nodes.len() - dropped);
    unraised.validate().unwrap();
    let error = printable(&m, Some(&unraised), None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
    assert_eq!(error.elements(), ["Face4"], "{error}");
}

#[test]
fn a_raised_face_with_no_unraised_mesh_or_another_shapes_is_a_bug() {
    let (m, unraised) = loose_tetra(&[(3, 0.06)]);
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal, "{error}");
    assert!(error.message().contains("no unraised mesh"), "{error}");
    let others: [fn(&mut ExportMesh); 4] = [
        |u| u.force_face_deflection = false,
        |u| u.tolerance = 0.1f64.next_up(),
        |u| u.deflection = 0.1f64.next_down(),
        |u| u.face_numbers.swap(0, 1),
    ];
    for (k, other) in others.iter().enumerate() {
        let mut u = unraised.clone();
        other(&mut u);
        let error = printable(&m, Some(&u), None).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal, "{k}: {error}");
        assert!(error.message().contains("another's"), "{k}: {error}");
    }
    // One that doesn't hold together is refused before it's indexed.
    let mut broken = unraised.clone();
    broken.face_nodes[0] += 1;
    let error = printable(&m, Some(&broken), None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.message().contains("unraised"), "{error}");
}

#[test]
fn the_tolerance_suggested_for_a_raised_face_is_whole_nanometres_the_worker_wont_raise_at() {
    // Through printable: the margin the worker meshed with (tolerance less deflection) is
    // the next export's too, so the suggestion less it must be at least twice the face's
    // tolerance, and a whole number of nanometres no more than one past the least such.
    for (tolerance, margin, loose) in [
        (0.1, 3e-7, 0.06),
        (0.01, 5.16e-6, 0.02),
        (1.0, 1.7e-4, 0.7),
        (0.5, 1e-12, 0.333_333_333_3),
    ] {
        let (mut m, mut unraised) = loose_tetra(&[(3, loose)]);
        for r in [&mut m, &mut unraised] {
            r.tolerance = tolerance;
            r.deflection = tolerance - margin;
            for d in &mut r.edge_deflections {
                *d = 0.0;
            }
        }
        let face4 = unraised.face_ranges()[3].start as usize;
        unraised.nodes[face4][2] += 1e-3;
        let error = printable(&m, Some(&unraised), None).unwrap_err();
        let suggestion = error.suggestion().unwrap();
        let suggested: f64 = suggestion
            .strip_prefix("export with a tolerance of at least ")
            .and_then(|s| s.split_once(" mm"))
            .unwrap()
            .0
            .parse()
            .unwrap();
        let margin = tolerance - (tolerance - margin);
        let nanometres = suggested * 1e6;
        assert_eq!(nanometres, nanometres.round(), "{suggested}");
        assert!(suggested - margin >= 2.0 * loose, "{suggested}");
        assert!(
            suggested <= 2.0 * loose + margin + 2e-6,
            "{suggested} for {loose} and {margin}"
        );
    }
}
