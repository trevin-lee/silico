//! M4.8's export core on meshes built by hand (docs/ports/mesh-export.md): a tetrahedron
//! joined, checked, and written, and each rule's refusal on a mesh broken to fail it. The
//! real kernel's meshes are silico-kernel's tests/export.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::export_report::{P, TETRA, refused, report, tetra};

use silico_core::ErrorCode;
use silico_core::export::{Object, PrintableMesh, printable, three_mf};
use silico_protocol::ExportMesh;

#[test]
fn a_tetrahedron_joins_into_four_vertices_and_encloses_a_sixth() {
    let mesh = printable(&tetra(), None).unwrap();
    // Vertices as the triangles first name them: Face1's P0, P2, P1, then Face2's P3.
    assert_eq!(
        mesh.vertices(),
        [
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0]
        ]
    );
    assert_eq!(
        mesh.triangles(),
        [[0, 1, 2], [0, 2, 3], [0, 3, 1], [2, 1, 3]]
    );
    // Only the slanted face is off the origin: det(P1, P2, P3) / 6.
    assert_eq!(mesh.volume(), 1.0 / 6.0);
}

#[test]
fn a_face_the_kernel_couldnt_mesh_is_refused_naming_it() {
    let mut m = tetra();
    // Face4 with no triangles, nodes, or polygons, as the worker reports an unmeshed face.
    m.triangles.pop();
    m.face_triangles[3] = 0;
    m.validate().unwrap();
    refused(
        &m,
        ErrorCode::InvalidResult,
        "couldn't mesh 1 of the faces",
        &["Face4"],
    );
}

#[test]
fn an_open_mesh_is_refused_naming_the_faces_at_its_hole() {
    // Without Face4, an edge of each other face is on one triangle.
    let m = report(&P, &TETRA[..3]);
    refused(
        &m,
        ErrorCode::InvalidResult,
        "the mesh is open",
        &["Face1", "Face2", "Face3"],
    );
}

#[test]
fn an_edge_on_three_triangles_is_refused_as_not_manifold() {
    // Face4 twice: each of its edges is on three triangles.
    let m = report(&P, &[TETRA[0], TETRA[1], TETRA[2], TETRA[3], TETRA[3]]);
    refused(
        &m,
        ErrorCode::InvalidResult,
        "isn't manifold",
        &["Face1", "Face2", "Face3", "Face4", "Face5"],
    );
}

#[test]
fn a_face_wound_the_wrong_way_is_refused_naming_it_and_its_neighbours() {
    let m = report(&P, &[TETRA[0], TETRA[1], TETRA[2], &[[1, 3, 2]]]);
    refused(
        &m,
        ErrorCode::InvalidResult,
        "oriented inconsistently",
        &["Face1", "Face2", "Face3", "Face4"],
    );
}

#[test]
fn a_shell_facing_inward_is_refused_and_a_cavity_must_face_inward() {
    let inward: Vec<[[usize; 3]; 1]> = TETRA
        .iter()
        .map(|f| [[f[0][0], f[0][2], f[0][1]]])
        .collect();
    let faces: Vec<&[[usize; 3]]> = inward.iter().map(|f| f.as_slice()).collect();
    let m = report(&P, &faces);
    refused(
        &m,
        ErrorCode::InvalidResult,
        "face inward",
        &["Face1", "Face2", "Face3", "Face4"],
    );
    // The same shell as a cavity's (not its solid's outer shell): enclosing a negative
    // volume is right for a cavity, and outward is wrong. But a solid that's only a cavity
    // encloses -1/6 in all, and is refused (break-it: it was taken, its volume negative).
    let mut cavity = m.clone();
    cavity.shell_outer = vec![false];
    refused(
        &cavity,
        ErrorCode::InvalidResult,
        "encloses no volume in all",
        &["Face1", "Face2", "Face3", "Face4"],
    );
    let mut outward_cavity = tetra();
    outward_cavity.shell_outer = vec![false];
    refused(
        &outward_cavity,
        ErrorCode::InvalidResult,
        "face inward",
        &["Face1", "Face2", "Face3", "Face4"],
    );
}

#[test]
fn a_triangle_flat_once_rounded_to_32_bits_but_with_its_corners_apart_is_written() {
    // The base split by M, a hair outside its slanted edge P1P2: (0.5 + 1e-9, 0.5 + 1e-9,
    // 0), which rounds to (0.5, 0.5, 0) in 32 bits (their spacing at 0.5 is 6e-8), on the
    // edge. The sliver (P1, P2, M), part of Face1, has area in 64 bits and none in 32, its
    // corners still apart; the slanted face is split at M to meet it. Closed, oriented, and
    // enclosing a sixth. 3MF asks only that a triangle SHOULD have area (core.md:588), and
    // FreeCAD writes such needles, as OCCT leaves on a cone at times: written, not refused
    // (break-it: NIST's CTC 01 was refused at 0.1 mm for them). Corners at one place are
    // refused (the test after).
    let mut points = P.to_vec();
    points.push([0.5 + 1e-9, 0.5 + 1e-9, 0.0]);
    let m = report(
        &points,
        &[
            &[[0, 2, 1], [1, 2, 4]],
            TETRA[1],
            TETRA[2],
            &[[1, 4, 3], [4, 2, 3]],
        ],
    );
    let mesh = printable(&m, None).unwrap();
    assert_eq!(mesh.triangles().len(), 6);
    // The same 1e-6 out is a triangle with area in 32 bits too, and passes.
    let mut points = P.to_vec();
    points.push([0.5 + 1e-6, 0.5 + 1e-6, 0.0]);
    let m = report(
        &points,
        &[
            &[[0, 2, 1], [1, 2, 4]],
            TETRA[1],
            TETRA[2],
            &[[1, 4, 3], [4, 2, 3]],
        ],
    );
    assert!(printable(&m, None).is_ok());
}

#[test]
fn faces_whose_polygons_on_an_edge_dont_pair_are_refused_naming_them() {
    let mut m = tetra();
    // Face4's polygon on P1P2 (its first edge use, sorted (1, 2)) given a third node, its own
    // P3's: it no longer pairs one for one with Face1's.
    let use_at = (0..m.edge_faces.len())
        .find(|&k| m.edge_faces[k] == 3 && m.edge_vertices[k] == [2, 3])
        .unwrap();
    let start: u32 = m.edge_node_counts[..use_at].iter().sum();
    let extra = m.edge_nodes[start as usize + 1];
    m.edge_nodes.insert(start as usize + 1, extra);
    m.edge_node_counts[use_at] = 3;
    m.validate().unwrap();
    refused(
        &m,
        ErrorCode::InvalidResult,
        "don't meet in the mesh",
        &["Face1", "Face4"],
    );
    // And a polygon missing altogether.
    let mut m = tetra();
    let start: u32 = m.edge_node_counts[..use_at].iter().sum();
    m.edge_nodes.drain(start as usize..start as usize + 2);
    m.edge_node_counts[use_at] = 0;
    m.validate().unwrap();
    refused(
        &m,
        ErrorCode::InvalidResult,
        "don't meet in the mesh",
        &["Face1", "Face4"],
    );
}

#[test]
fn deviations_past_the_tolerance_less_the_rounding_are_refused() {
    // The tetrahedron reaches 1 mm: rounding moves it by up to √3 · 2⁻²⁴ ≈ 1.03e-7 mm, so
    // an edge's 0.1 − 0.5e-7 is past what's allowed, and 0.1 − 1.5e-7 within it.
    let rounding = 3f64.sqrt() / 16_777_216.0;
    let mut m = tetra();
    m.edge_deflections[0] = 0.1 - rounding * 0.5;
    refused(
        &m,
        ErrorCode::InvalidResult,
        "further than the tolerance",
        &["Face1"],
    );
    let mut m = tetra();
    m.edge_deflections[0] = 0.1 - rounding * 1.5;
    assert!(printable(&m, None).is_ok());
    // A face's own deviation, as OCCT estimates it, isn't held to the tolerance, as FreeCAD
    // takes it: the estimate overstates a cone's by 10 to 1000 times (break-it).
    let mut m = tetra();
    m.face_deflections[1] = 1.0;
    assert!(printable(&m, None).is_ok());
    // An edge use's deflection raised past it names its face.
    let mut m = tetra();
    m.edge_deflections[0] = 0.1;
    refused(
        &m,
        ErrorCode::InvalidResult,
        "further than the tolerance",
        &["Face1"],
    );
    // A node joined into a vertex 0.2 mm from where the vertex is (Face4's P1, the first
    // node at P1 being Face1's).
    let mut m = tetra();
    let p1_of_face4 = m.face_ranges()[3].start;
    assert_eq!(m.nodes[p1_of_face4 as usize], P[1]);
    m.nodes[p1_of_face4 as usize] = [1.2, 0.0, 0.0];
    refused(
        &m,
        ErrorCode::InvalidResult,
        "further than the tolerance",
        &["Face1", "Face4"],
    );
}

#[test]
fn shapes_without_solids_and_faces_outside_them_are_refused() {
    // A shape of no solid at all, as a face alone reports it: no face in a solid.
    let m = ExportMesh {
        tolerance: 0.1,
        deflection: 0.1,
        loose_faces: vec![1],
        ..ExportMesh::default()
    };
    m.validate().unwrap();
    let error = printable(&m, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.message().contains("no solid"), "{error}");
    let mut m = tetra();
    m.loose_faces = vec![7, 9];
    refused(
        &m,
        ErrorCode::InvalidInput,
        "in no solid",
        &["Face7", "Face9"],
    );
}

#[test]
fn a_report_of_a_tolerance_outside_the_exports_range_is_refused() {
    // The tolerance a report was meshed for is checked with the report: a request for one
    // outside the range is refused before it's sent (KernelRequest::export_mesh).
    for bad in [1e-6, 0.0, -1.0, f64::NAN, 1e9] {
        let mut m = tetra();
        m.tolerance = bad;
        let error = printable(&m, None).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{bad}: {error}");
        assert!(
            error.message().contains("doesn't hold together"),
            "{bad}: {error}"
        );
        assert!(
            error.cause().is_some_and(|c| c.contains("tolerance")),
            "{bad}: {error}"
        );
    }
}

#[test]
fn two_solids_are_joined_each_on_its_own() {
    // Two tetrahedra sharing the corner P0 and the vertex numbers, in two solids: no
    // vertex of one is the other's, so 8 vertices.
    let one = tetra();
    let mut two = tetra();
    let offset = two.nodes.len() as u32;
    two.solids = 2;
    two.shell_solids = vec![0, 1];
    two.shell_outer = vec![true, true];
    let faces = one.face_nodes.len() as u32;
    two.nodes.extend(&one.nodes);
    two.face_nodes.extend(&one.face_nodes);
    two.triangles
        .extend(one.triangles.iter().map(|t| t.map(|n| n + offset)));
    two.face_triangles.extend(&one.face_triangles);
    two.face_numbers
        .extend(one.face_numbers.iter().map(|n| n + faces));
    two.face_deflections.extend(&one.face_deflections);
    two.face_shells.extend(one.face_shells.iter().map(|_| 1));
    two.edge_faces
        .extend(one.edge_faces.iter().map(|f| f + faces));
    two.edge_numbers.extend(&one.edge_numbers);
    two.edge_degenerate.extend(&one.edge_degenerate);
    two.edge_vertices.extend(&one.edge_vertices);
    two.edge_deflections.extend(&one.edge_deflections);
    two.edge_node_counts.extend(&one.edge_node_counts);
    two.edge_nodes
        .extend(one.edge_nodes.iter().map(|n| n + offset));
    two.validate().unwrap();
    let mesh = printable(&two, None).unwrap();
    assert_eq!(mesh.vertices().len(), 8);
    assert_eq!(mesh.triangles().len(), 8);
    assert_eq!(mesh.volume(), 2.0 / 6.0);
}

/// The archive's entries: each name and its data, read from the local headers, with each
/// header's fields checked against the zip format (APPNOTE 6.3.3, 4.3.7) as this writer
/// promises to fill them.
fn entries(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    let u16_at = |at: usize| u16::from_le_bytes([archive[at], archive[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(archive[at..at + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut at = 0;
    while u32_at(at) == 0x0403_4b50 {
        assert_eq!(u16_at(at + 4), 10, "version needed");
        assert_eq!(u16_at(at + 6), 0, "flags");
        assert_eq!(u16_at(at + 8), 0, "stored");
        assert_eq!(u16_at(at + 10), 0, "time 00:00");
        assert_eq!(u16_at(at + 12), 0x21, "date 1980-01-01");
        let (compressed, size) = (u32_at(at + 18), u32_at(at + 22));
        assert_eq!(compressed, size);
        let name_len = usize::from(u16_at(at + 26));
        assert_eq!(u16_at(at + 28), 0, "extra field");
        let name = String::from_utf8(archive[at + 30..at + 30 + name_len].to_vec()).unwrap();
        let data_at = at + 30 + name_len;
        let data = archive[data_at..data_at + size as usize].to_vec();
        out.push((name, data));
        at = data_at + size as usize;
    }
    // The central directory, then its end, which says where the directory is and that
    // there are as many entries.
    assert_eq!(u32_at(at), 0x0201_4b50);
    let end = archive.len() - 22;
    assert_eq!(u32_at(end), 0x0605_4b50);
    assert_eq!(usize::from(u16_at(end + 10)), out.len());
    assert_eq!(u32_at(end + 16) as usize, at);
    assert_eq!(u32_at(end + 12) as usize, end - at);
    out
}

fn tetra_mesh() -> PrintableMesh {
    printable(&tetra(), None).unwrap()
}

#[test]
fn a_3mf_holds_the_three_entries_in_order_and_the_model_as_written_by_hand() {
    let mesh = tetra_mesh();
    let archive = three_mf(&[Object {
        name: "Tetra <1> & \"2\"",
        part_number: "k7f2qx3m",
        mesh: &mesh,
    }])
    .unwrap();
    let found = entries(&archive);
    let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model"]
    );
    let model = String::from_utf8(found[2].1.clone()).unwrap();
    assert_eq!(
        model,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <metadata name="Application">Silico</metadata>
 <resources>
  <object id="1" type="model" name="Tetra &lt;1&gt; &amp; &quot;2&quot;" partnumber="k7f2qx3m">
   <mesh>
    <vertices>
     <vertex x="0" y="0" z="0"/>
     <vertex x="0" y="1" z="0"/>
     <vertex x="1" y="0" z="0"/>
     <vertex x="0" y="0" z="1"/>
    </vertices>
    <triangles>
     <triangle v1="0" v2="1" v3="2"/>
     <triangle v1="0" v2="2" v3="3"/>
     <triangle v1="0" v2="3" v3="1"/>
     <triangle v1="2" v2="1" v3="3"/>
    </triangles>
   </mesh>
  </object>
 </resources>
 <build>
  <item objectid="1"/>
 </build>
</model>
"#
    );
    let rels = String::from_utf8(found[1].1.clone()).unwrap();
    assert!(rels.contains(r#"Target="/3D/3dmodel.model""#), "{rels}");
    assert!(
        rels.contains(r#"Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel""#),
        "{rels}"
    );
    let types = String::from_utf8(found[0].1.clone()).unwrap();
    assert!(
        types.contains(
            r#"<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>"#
        ),
        "{types}"
    );
    // The same objects give the same bytes.
    assert_eq!(
        archive,
        three_mf(&[Object {
            name: "Tetra <1> & \"2\"",
            part_number: "k7f2qx3m",
            mesh: &mesh,
        }])
        .unwrap()
    );
}

#[test]
fn coordinates_are_the_shortest_decimal_of_their_32_bit_value_and_minus_zero_is_zero() {
    let mut points = P.to_vec();
    for p in &mut points {
        // 0.1 as 32 bits is 0.100000001490116…, written "0.1"; 1/3 is "0.33333334".
        *p = [p[0] * 0.1 - 0.0, p[1] / 3.0, -p[2] * 0.0];
    }
    points[3] = [0.0, 0.0, 1.0 / 3.0];
    let mesh = printable(&report(&points, &TETRA), None).unwrap();
    let archive = three_mf(&[Object {
        name: "T",
        part_number: "p",
        mesh: &mesh,
    }])
    .unwrap();
    let model = String::from_utf8(entries(&archive)[2].1.clone()).unwrap();
    assert!(
        model.contains(r#"<vertex x="0" y="0.33333334" z="0"/>"#),
        "{model}"
    );
    assert!(
        model.contains(r#"<vertex x="0.1" y="0" z="0"/>"#),
        "{model}"
    );
    assert!(
        model.contains(r#"<vertex x="0" y="0" z="0.33333334"/>"#),
        "{model}"
    );
    assert!(!model.contains("-0"), "{model}");
}

#[test]
fn a_name_xml_cant_hold_is_refused_naming_the_object() {
    let mesh = tetra_mesh();
    let error = three_mf(&[Object {
        name: "bad\u{1}name",
        part_number: "p",
        mesh: &mesh,
    }])
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.message().contains("control character"), "{error}");
    assert_eq!(error.elements(), ["bad\u{1}name"]);
}

#[test]
fn faces_at_fault_are_named_as_the_features_element_map_names_them() {
    use silico_core::document::FeatureId;
    use silico_core::naming::{ElementMap, OpCode};
    let map = ElementMap::by_number(
        FeatureId::new("k7f2qx3m").unwrap(),
        OpCode::Cut,
        silico_protocol::ElementCounts {
            faces: 4,
            edges: 6,
            vertices: 4,
            solids: 1,
        },
    );
    let mut m = tetra();
    m.triangles.pop();
    m.face_triangles[3] = 0;
    let error = printable(&m, Some(&map)).unwrap_err();
    assert_eq!(error.elements(), ["k7f2qx3m.cut.Face4"], "{error}");
}

#[test]
fn vertices_apart_in_64_bits_but_one_in_32_are_refused_before_anything_is_written() {
    // M at (1 + 1e-9, 0, 0), beside P1 (1, 0, 0) and outside the slanted edge P1P2: distinct
    // in 64 bits, and P1 exactly in
    // the 32 a reader parses (their spacing at 1 is 1.2e-7). The sliver (P1, P2, M),
    // Face1's, and Face4's (P1, M, P3) each have two corners at one place once rounded, so no
    // area: the mesh is refused, not written with vertices its readers would read as one.
    let mut points = P.to_vec();
    points.push([1.0 + 1e-9, 0.0, 0.0]);
    let m = report(
        &points,
        &[
            &[[0, 2, 1], [1, 2, 4]],
            TETRA[1],
            TETRA[2],
            &[[1, 4, 3], [4, 2, 3]],
        ],
    );
    assert_ne!(points[4], points[1]);
    assert_eq!(points[4].map(|c| c as f32), points[1].map(|c| c as f32));
    refused(
        &m,
        ErrorCode::InvalidResult,
        "two corners at one place",
        &["Face1", "Face4"],
    );
}

#[test]
fn each_object_is_its_own_mesh_with_its_own_id_and_build_item() {
    // The council's tests lens: every build item's objectid, and each object's own
    // vertices, not the first object's.
    let a = tetra_mesh();
    let big: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| 2.0 * c)).collect();
    let b = printable(&report(&big, &TETRA), None).unwrap();
    let archive = three_mf(&[
        Object {
            name: "A",
            part_number: "a",
            mesh: &a,
        },
        Object {
            name: "B",
            part_number: "b",
            mesh: &b,
        },
    ])
    .unwrap();
    let model = String::from_utf8(entries(&archive)[2].1.clone()).unwrap();
    let (first, second) = model
        .split_once(r#"<object id="2" type="model" name="B" partnumber="b">"#)
        .unwrap();
    assert!(first.contains(r#"<object id="1" type="model" name="A" partnumber="a">"#));
    assert!(first.contains(r#"<vertex x="1" y="0" z="0"/>"#), "{first}");
    assert!(!first.contains(r#"x="2""#), "{first}");
    assert!(
        second.contains(r#"<vertex x="2" y="0" z="0"/>"#),
        "{second}"
    );
    assert!(
        model.ends_with(
            " <build>\n  <item objectid=\"1\"/>\n  <item objectid=\"2\"/>\n </build>\n</model>\n"
        ),
        "{model}"
    );
}

#[test]
fn faces_that_share_no_edge_are_never_joined_however_close_their_nodes() {
    // The ported half of FreeCAD's testUnconnectedDomains that Silico departs on
    // (tests/src/Mod/Part/App/BRepMesh.cpp:166, eb32abe7ef): FreeCAD merges nodes 1e-10
    // apart; Silico joins faces only at the B-rep edges they share. Face4 given edges and
    // vertices of its own, at the same places (0 apart) or 1e-10 apart: the mesh is open.
    for offset in [0.0, 1e-10] {
        let mut m = tetra();
        let top = *m.edge_numbers.iter().max().unwrap();
        for k in 0..m.edge_faces.len() {
            if m.edge_faces[k] == 3 {
                m.edge_numbers[k] += top;
                m.edge_vertices[k] = m.edge_vertices[k].map(|v| v + 4);
            }
        }
        for n in m.face_ranges()[3].clone() {
            m.nodes[n as usize][0] += offset;
        }
        m.validate().unwrap();
        refused(
            &m,
            ErrorCode::InvalidResult,
            "the mesh is open",
            &["Face1", "Face2", "Face3", "Face4"],
        );
    }
}
