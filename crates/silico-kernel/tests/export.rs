//! M4.8's export core on OCCT's meshes (docs/ports/mesh-export.md): solids meshed within
//! the export tolerance and its angular rule, joined at the edges their faces share, checked
//! printable, and written as 3MF whose bytes are the same every time and on every platform.
//! Meshes built by hand, one per rule, are silico-core's tests/export.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::exported;
use super::kernel;

use exported::{data, exported};
use kernel::kernel;
use sha2::{Digest, Sha256};
use silico_core::ErrorCode;
use silico_core::export::{Object, PrintableMesh, printable, three_mf};
use silico_kernel::{
    BooleanOp, KernelErrorKind, Primitive, boolean, export_mesh, make, make_box, to_brep, translate,
};
use silico_protocol::ChamferSize;

fn written(mesh: &PrintableMesh) -> Vec<u8> {
    three_mf(&[Object {
        name: "Part",
        part_number: "k7f2qx3m",
        mesh,
    }])
    .unwrap()
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn a_boxs_mesh_is_its_8_corners_12_triangles_and_exactly_its_volume() {
    let mesh = exported(&make_box(10.0, 20.0, 30.0).unwrap(), 0.1).unwrap();
    assert_eq!(mesh.triangles().len(), 12);
    let mut corners: Vec<[f32; 3]> = mesh.vertices().to_vec();
    corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut expected = Vec::new();
    for x in [0.0, 10.0] {
        for y in [0.0, 20.0] {
            for z in [0.0, 30.0] {
                expected.push([x, y, z]);
            }
        }
    }
    assert_eq!(corners, expected);
    assert_eq!(mesh.volume(), 6000.0);
}

#[test]
fn a_cylinders_mesh_is_a_prism_set_by_the_angular_rule() {
    // r 5, h 10 at 0.1 mm: FreeCAD's angular deflection there, 0.1 rad, needs at least
    // ⌈2π / 0.1⌉ = 63 sides; the 0.1 mm alone would take 16 (n ≥ π / acos(1 − 0.1/5)).
    let (r, h) = (5.0f64, 10.0f64);
    let cylinder = make(&Primitive::Cylinder {
        radius: r,
        height: h,
    })
    .unwrap();
    let mesh = exported(&cylinder, 0.1).unwrap();
    let on_cap = |z: f32| {
        mesh.vertices()
            .iter()
            .filter(|v| {
                v[2] == z && (libm::hypot(f64::from(v[0]), f64::from(v[1])) - r).abs() < 1e-5
            })
            .count()
    };
    let n = on_cap(0.0);
    assert!(n >= 63, "{n} sides");
    assert_eq!(on_cap(10.0), n);
    // Every vertex is on a cap's circle: the mesh is the inscribed prism, of volume
    // (n/2)·r²·h·sin(2π/n), to the 32-bit rounding of its corners (about 6e-8 each).
    assert_eq!(mesh.vertices().len(), 2 * n);
    let prism = n as f64 / 2.0 * r * r * h * libm::sin(std::f64::consts::TAU / n as f64);
    assert!(
        (mesh.volume() - prism).abs() <= 1e-6 * prism,
        "{} against {prism}",
        mesh.volume()
    );
    assert!(mesh.volume() < std::f64::consts::PI * r * r * h);
    // n ≥ 63 gives at least 784.097.
    assert!(mesh.volume() >= 784.097, "{}", mesh.volume());
}

#[test]
fn a_spheres_poles_join_and_the_mesh_closes() {
    let r = 5.0f64;
    let sphere = make(&Primitive::Sphere { radius: r }).unwrap();
    let mesh = exported(&sphere, 0.1).unwrap();
    // Inscribed: every vertex within the 32-bit rounding of the sphere, so the volume is
    // under the sphere's, and over that of the sphere 0.1 mm smaller (each triangle's sagitta
    // is within the tolerance).
    for v in mesh.vertices() {
        let d = v
            .iter()
            .map(|&c| f64::from(c) * f64::from(c))
            .sum::<f64>()
            .sqrt();
        assert!((d - r).abs() <= 1e-6, "{v:?} at {d}");
    }
    let ball = |r: f64| 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
    assert!(mesh.volume() < ball(r), "{}", mesh.volume());
    assert!(mesh.volume() > ball(r - 0.1), "{}", mesh.volume());
    // Each pole one vertex (OCCT's is a rounding off the axis): the seam and the degenerate
    // edges joined.
    for z in [r as f32, -r as f32] {
        let at = mesh
            .vertices()
            .iter()
            .filter(|v| {
                let d = [v[0], v[1], v[2] - z].map(|c| f64::from(c) * f64::from(c));
                (d[0] + d[1] + d[2]).sqrt() <= 1e-6
            })
            .count();
        assert_eq!(at, 1, "pole {z}");
    }
}

#[test]
fn two_solids_touching_along_an_edge_are_two_closed_meshes_sharing_no_vertex() {
    let cube = make_box(1.0, 1.0, 1.0).unwrap();
    let other = translate(&cube, [1.0, 1.0, 0.0]).unwrap().shape;
    let two = boolean(BooleanOp::Fuse, &cube, &other).unwrap().shape;
    let mesh = exported(&two, 0.1).unwrap();
    assert_eq!(mesh.vertices().len(), 16);
    assert_eq!(mesh.triangles().len(), 24);
    // Summed about the origin, the cube off it adds terms of both signs: 2 to rounding.
    assert!((mesh.volume() - 2.0).abs() <= 2e-12, "{}", mesh.volume());
}

#[test]
fn a_solid_with_faces_the_kernel_cant_mesh_is_refused_naming_them() {
    // A 1 × 2 × 3 box chamfered nearly through: its slivers Face8 and Face13 are too narrow
    // for OCCT to mesh at any tolerance (adversarial_mesh.rs), as FreeCAD finds them. The
    // display leaves them out; an export would have holes there.
    let block = make_box(1.0, 2.0, 3.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let built = silico_kernel::chamfer(
        &block,
        &all,
        ChamferSize::Equal {
            distance: 0.499_999_9,
        },
        false,
    )
    .unwrap();
    for tolerance in [0.1, 1e-4] {
        let error = exported(&built.shape, tolerance).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
        assert_eq!(error.elements(), ["Face8", "Face13"], "{error}");
        assert_eq!(error.elements_total(), 2, "{error}");
    }
}

#[test]
fn a_part_too_far_out_for_the_tolerance_in_32_bits_is_refused_naming_the_tolerance() {
    // A 10 mm cube 5·10⁴ mm out: rounding to 32 bits moves it by up to √3·5.001e4·2⁻²⁴ ≈
    // 5.16e-3 mm, more than a 1e-3 mm tolerance. At 0.1 mm it exports, its corners exact
    // in 32 bits (multiples of 2⁻⁸ there).
    let far = translate(&make_box(10.0, 10.0, 10.0).unwrap(), [5e4, 0.0, 0.0])
        .unwrap()
        .shape;
    let refused = export_mesh(&far, 1e-3).unwrap_err();
    assert_eq!(refused.kind(), KernelErrorKind::InvalidInput, "{refused}");
    let error = exported(&far, 1e-3).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
    assert!(error.message().contains("32 bits"), "{error}");
    let mesh = exported(&far, 0.1).unwrap();
    // Summed about the origin, 5·10⁴ mm away: 1000 to the rounding of terms 10⁵ times it.
    assert!((mesh.volume() - 1000.0).abs() <= 1e-9, "{}", mesh.volume());
}

#[test]
fn a_tolerance_outside_the_exports_range_is_refused_before_its_sent() {
    let cube = data(&make_box(1.0, 1.0, 1.0).unwrap());
    for bad in [1e-6, 0.0, -0.1, f64::NAN, 1e9] {
        let Err(error) = silico_core::kernel::KernelRequest::export_mesh(&cube, bad) else {
            panic!("{bad} was taken");
        };
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{bad}: {error}");
        assert_eq!(error.parameter(), Some("tolerance"), "{bad}: {error}");
    }
}

#[test]
fn exporting_leaves_the_shape_as_it_was() {
    let cylinder = make(&Primitive::Cylinder {
        radius: 5.0,
        height: 10.0,
    })
    .unwrap();
    let before = to_brep(&cylinder).unwrap();
    export_mesh(&cylinder, 0.1).unwrap();
    assert_eq!(to_brep(&cylinder).unwrap(), before);
}

#[test]
fn a_shape_without_a_solid_is_refused() {
    // A triangle's face, with no solid.
    let faces = silico_kernel::make_faces(&silico_protocol::FaceRegions {
        plane: silico_protocol::SketchPlane::Xy,
        regions: vec![silico_protocol::Region {
            outer: vec![
                silico_protocol::Curve::Line {
                    start: [0.0, 0.0],
                    end: [1.0, 0.0],
                },
                silico_protocol::Curve::Line {
                    start: [1.0, 0.0],
                    end: [0.0, 1.0],
                },
                silico_protocol::Curve::Line {
                    start: [0.0, 1.0],
                    end: [0.0, 0.0],
                },
            ],
            holes: vec![],
        }],
    })
    .unwrap()
    .shape;
    let error = exported(&faces, 0.1).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.message().contains("no solid"), "{error}");
}

#[test]
fn an_export_is_the_same_bytes_every_time_and_on_every_platform() {
    // The digests pin the bytes on every platform CI checks (macOS ARM, Linux x86): the
    // same shape, through OCCT's mesher and the writer, gives the same file.
    let cylinder = make(&Primitive::Cylinder {
        radius: 5.0,
        height: 10.0,
    })
    .unwrap();
    for (shape, digest) in [
        (
            make_box(10.0, 20.0, 30.0).unwrap(),
            "ab028ad17304f341352b15a80318855ac08aa8241e0346d2d974c0d2343e0719",
        ),
        (
            cylinder,
            "cda466128befd11902210c565bab88d94b5484bb4b5e54c0ce8865c7373e800a",
        ),
        // A sphere: its seam's band is where OCCT's Delaunay meshes irregularly
        // (m4.8-export-fable, item 2), so the likeliest place for platforms to differ.
        (
            make(&Primitive::Sphere { radius: 50.0 }).unwrap(),
            "545173fd17c1b6e77fd73be89470a3185b58bbf67cf6b68788a8413499733889",
        ),
    ] {
        let first = written(&exported(&shape, 0.1).unwrap());
        let again = written(&exported(&shape, 0.1).unwrap());
        assert_eq!(first, again);
        assert_eq!(hex(&first), digest);
    }
}

#[test]
fn a_box_with_a_cavity_is_two_shells_the_cavitys_facing_in() {
    // 10 mm cube less a 2 mm cube inside it: one solid, two shells, 1000 − 8.
    let outer = make_box(10.0, 10.0, 10.0).unwrap();
    let inner = translate(&make_box(2.0, 2.0, 2.0).unwrap(), [4.0, 4.0, 4.0])
        .unwrap()
        .shape;
    let hollow = boolean(BooleanOp::Cut, &outer, &inner).unwrap().shape;
    let raw = kernel().export_mesh(&data(&hollow), 0.1).unwrap();
    assert_eq!(raw.solids, 1);
    assert_eq!(raw.shell_outer.iter().filter(|&&o| o).count(), 1);
    assert_eq!(raw.shell_outer.len(), 2);
    let mesh = printable(&raw, None).unwrap();
    assert_eq!(mesh.vertices().len(), 16);
    assert_eq!(mesh.triangles().len(), 24);
    assert!(
        (mesh.volume() - 992.0).abs() <= 1e-12 * 992.0,
        "{}",
        mesh.volume()
    );
}

/// What FreeCAD 1.1.3 reads from the 3MF at `path`: whether its mesh is a solid (no edge
/// without a neighbour), has no non-manifold edge, and every facet is oriented as its
/// neighbours; its points and facets; and its volume, summed in doubles over its facets as
/// Silico sums it (a tetrahedron from the origin each), not FreeCAD's 32-bit unsigned
/// `Volume`.
fn freecad_reads(path: &std::path::Path) -> serde_json::Value {
    super::oracle::freecad(&format!(
        r#"
import json, Mesh
m = Mesh.Mesh({path:?})
points, facets = m.Topology
volume = 0.0
for (i, j, k) in facets:
    a, b, c = points[i], points[j], points[k]
    cross = (b.y * c.z - b.z * c.y, b.z * c.x - b.x * c.z, b.x * c.y - b.y * c.x)
    volume += (a.x * cross[0] + a.y * cross[1] + a.z * cross[2]) / 6.0
print("SILICO_ORACLE " + json.dumps({{
    "solid": m.isSolid(),
    "non_manifold": m.hasNonManifolds(),
    "misoriented": m.countNonUniformOrientedFacets(),
    "points": m.CountPoints,
    "facets": m.CountFacets,
    "volume": volume,
}}))
"#,
        path = path.to_str().unwrap()
    ))
}

#[test]
fn freecad_reads_each_export_back_as_a_solid_with_the_volume_silico_measured() {
    let dir = tempfile::Builder::new()
        .prefix("silico-export-")
        .tempdir()
        .unwrap();
    let block = make_box(10.0, 20.0, 30.0).unwrap();
    let filleted = silico_kernel::fillet(&block, &[1, 2, 3], 2.0)
        .unwrap()
        .shape;
    let rod = translate(
        &make(&Primitive::Cylinder {
            radius: 2.0,
            height: 30.0,
        })
        .unwrap(),
        [5.0, 10.0, 0.0],
    )
    .unwrap()
    .shape;
    let drilled = boolean(BooleanOp::Cut, &block, &rod).unwrap().shape;
    let hollow = boolean(
        BooleanOp::Cut,
        &make_box(10.0, 10.0, 10.0).unwrap(),
        &translate(&make_box(2.0, 2.0, 2.0).unwrap(), [4.0, 4.0, 4.0])
            .unwrap()
            .shape,
    )
    .unwrap()
    .shape;
    let parts = [
        ("box", block),
        (
            "cylinder",
            make(&Primitive::Cylinder {
                radius: 5.0,
                height: 10.0,
            })
            .unwrap(),
        ),
        ("sphere", make(&Primitive::Sphere { radius: 5.0 }).unwrap()),
        ("filleted", filleted),
        ("drilled", drilled),
        ("hollow", hollow),
        ("touching", {
            let cube = make_box(1.0, 1.0, 1.0).unwrap();
            let other = translate(&cube, [1.0, 1.0, 0.0]).unwrap().shape;
            boolean(BooleanOp::Fuse, &cube, &other).unwrap().shape
        }),
    ];
    for (name, shape) in parts {
        let mesh = exported(&shape, 0.1).unwrap();
        let path = dir.path().join(format!("{name}.3mf"));
        std::fs::write(&path, written(&mesh)).unwrap();
        let read = freecad_reads(&path);
        assert_eq!(read["solid"], true, "{name}: {read}");
        assert_eq!(read["non_manifold"], false, "{name}: {read}");
        assert_eq!(read["misoriented"], 0, "{name}: {read}");
        assert_eq!(read["points"], mesh.vertices().len(), "{name}: {read}");
        assert_eq!(read["facets"], mesh.triangles().len(), "{name}: {read}");
        let volume = read["volume"].as_f64().unwrap();
        assert!(
            (volume - mesh.volume()).abs() <= 1e-12 * mesh.volume(),
            "{name}: FreeCAD {volume}, Silico {}",
            mesh.volume()
        );
        assert!(volume > 0.0, "{name}: {volume}");
    }
}

/// What FreeCAD 1.1.3's own `Mesh.export` of `part` (a Python expression for a shape) at
/// `tolerance` gives, read back: its points and facets.
fn freecads_export(part: &str, tolerance: f64, path: &std::path::Path) -> (u64, u64) {
    let read = super::oracle::freecad(&format!(
        r#"
import json, FreeCAD, Mesh, Part
doc = FreeCAD.newDocument()
o = doc.addObject("Part::Feature", "P")
o.Shape = {part}
Mesh.export([o], {path:?}, tolerance={tolerance})
m = Mesh.Mesh({path:?})
print("SILICO_ORACLE " + json.dumps({{"points": m.CountPoints, "facets": m.CountFacets}}))
"#,
        path = path.to_str().unwrap()
    ));
    (
        read["points"].as_u64().unwrap(),
        read["facets"].as_u64().unwrap(),
    )
}

#[test]
fn silicos_meshes_are_freecads_own_exports_but_where_the_rounding_margin_meshes_finer() {
    // The council's tests and port lenses: the same parts, meshed by FreeCAD's own export
    // (Mesh.export, getFaces, in parallel) and by Silico's (serially, at the tolerance less
    // the rounding the part's box allows): the same points and facets at each tolerance.
    // Where the margin makes OCCT mesh finer, it's listed below.
    // The disc reaches 1000 mm: its margin, √3·1000·2⁻²⁴ ≈ 1.03e-4 mm, leaves OCCT 0.0099 mm
    // at 0.01, and the rim's circle takes ten more segments (3,992 facets to FreeCAD's 3,972).
    const FINER: [&str; 1] = ["disc at 0.01"];
    let dir = tempfile::Builder::new()
        .prefix("silico-export-counts-")
        .tempdir()
        .unwrap();
    let parts = [
        (
            "cylinder",
            make(&Primitive::Cylinder {
                radius: 5.0,
                height: 10.0,
            })
            .unwrap(),
            "Part.makeCylinder(5, 10)",
        ),
        (
            "sphere",
            make(&Primitive::Sphere { radius: 5.0 }).unwrap(),
            "Part.makeSphere(5)",
        ),
        (
            "disc",
            make(&Primitive::Cylinder {
                radius: 1000.0,
                height: 1.0,
            })
            .unwrap(),
            "Part.makeCylinder(1000, 1)",
        ),
        (
            "box",
            make_box(10.0, 20.0, 30.0).unwrap(),
            "Part.makeBox(10, 20, 30)",
        ),
    ];
    let mut finer = Vec::new();
    let mut wrong = Vec::new();
    for (name, shape, part) in &parts {
        for tolerance in [1e-2, 0.1, 1.0] {
            let mesh = exported(shape, tolerance).unwrap();
            let path = dir.path().join(format!("{name}-{tolerance}.3mf"));
            let (points, facets) = freecads_export(part, tolerance, &path);
            let ours = (mesh.vertices().len() as u64, mesh.triangles().len() as u64);
            if ours != (points, facets) {
                let case = format!("{name} at {tolerance}");
                if FINER.contains(&case.as_str()) && ours.1 > facets {
                    finer.push(case);
                } else {
                    wrong.push(format!(
                        "{case}: Silico {ours:?}, FreeCAD {:?}",
                        (points, facets)
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert_eq!(finer, FINER);
}

#[test]
fn a_box_moved_by_10_5_2_is_written_at_exactly_its_bounds() {
    // FreeCAD's TestSingleMesh (tests/src/Mod/Mesh/App/Exporter.cpp:78): a placed part's
    // bounds in the 3MF are where it's placed. Silico's translate sets a location, which the
    // export applies to each node once (FreeCAD fixed it applied twice or not at all,
    // 0f9eba7d96, b52694c87b, b1a96bde5f).
    let moved = translate(&make_box(10.0, 10.0, 10.0).unwrap(), [10.0, 5.0, 2.0])
        .unwrap()
        .shape;
    let mesh = exported(&moved, 0.1).unwrap();
    let low = [0, 1, 2].map(|i| {
        mesh.vertices()
            .iter()
            .map(|v| v[i])
            .fold(f32::MAX, f32::min)
    });
    let high = [0, 1, 2].map(|i| {
        mesh.vertices()
            .iter()
            .map(|v| v[i])
            .fold(f32::MIN, f32::max)
    });
    assert_eq!((low, high), ([10.0, 5.0, 2.0], [20.0, 15.0, 12.0]));
    assert!((mesh.volume() - 1000.0).abs() <= 1e-9, "{}", mesh.volume());
}

#[test]
fn a_box_beside_a_cylinder_is_two_objects_each_with_its_own_counts() {
    // FreeCAD's Test3MF's two objects (tests/src/Mod/Mesh/App/Importer.cpp:20): each object
    // its own mesh, counted on its own.
    let block = exported(&make_box(10.0, 20.0, 30.0).unwrap(), 0.1).unwrap();
    let rod = exported(
        &make(&Primitive::Cylinder {
            radius: 5.0,
            height: 10.0,
        })
        .unwrap(),
        0.1,
    )
    .unwrap();
    let archive = three_mf(&[
        Object {
            name: "Block",
            part_number: "b0000000",
            mesh: &block,
        },
        Object {
            name: "Rod",
            part_number: "r0000000",
            mesh: &rod,
        },
    ])
    .unwrap();
    let text = String::from_utf8_lossy(&archive);
    let model = &text[text.find("<model").unwrap()..];
    let (first, second) = model.split_once("<object id=\"2\"").unwrap();
    assert_eq!(first.matches("<vertex ").count(), 8);
    assert_eq!(first.matches("<triangle ").count(), 12);
    assert_eq!(second.matches("<vertex ").count(), 252);
    assert_eq!(second.matches("<triangle ").count(), 500);
}

#[test]
fn a_face_beside_a_solid_is_refused_naming_it() {
    // The council's tests lens: a compound of a box and a free face (as STEP files carry,
    // 11 of NIST's 33 parts), made by FreeCAD: the worker reports the face as in no solid,
    // and the export refuses it, naming it, rather than leave it out.
    let dir = tempfile::Builder::new()
        .prefix("silico-export-loose-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("loose.brep");
    super::oracle::freecad(&format!(
        r#"
import json, FreeCAD, Part
c = Part.Compound([Part.makeBox(1, 1, 1), Part.makePlane(1, 1, FreeCAD.Vector(5, 0, 0))])
c.exportBrep({path:?})
print("SILICO_ORACLE " + json.dumps({{"faces": len(c.Faces)}}))
"#,
        path = path.to_str().unwrap()
    ));
    let shape = silico_kernel::from_text_brep(&std::fs::read(&path).unwrap()).unwrap();
    let raw = kernel().export_mesh(&data(&shape), 0.1).unwrap();
    assert_eq!((raw.solids, raw.loose_faces.clone()), (1, vec![7]));
    let error = printable(&raw, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.elements(), ["Face7"], "{error}");
}
