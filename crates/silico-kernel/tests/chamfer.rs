//! M1.5: the chamfer (docs/ports/chamfer.md), checked against hand calculations, FreeCAD's
//! own tests, FreeCAD as an oracle (measures, and the history its element names state), and
//! through a worker. Also the rule chamfer and fillet share: an edge OCCT won't take is
//! refused, naming it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/chamfer_sizes.rs"]
mod chamfer_sizes;
#[path = "common/close.rs"]
mod close;
#[path = "common/facts.rs"]
mod facts;
#[path = "common/imports.rs"]
mod imports;
#[path = "common/kernel.rs"]
mod kernel;
#[path = "common/numbers.rs"]
mod numbers;
#[path = "common/oracle.rs"]
mod oracle;

use std::f64::consts::PI;
use std::path::Path;
use std::time::Duration;

use chamfer_sizes::{angled, equal, two};
use close::assert_close;
use facts::{Fact, facts};
use kernel::kernel;
use silico_core::ErrorCode;
use silico_core::worker::Worker;
use silico_kernel::{
    Built, ChamferSize, ElementKind, KernelError, KernelErrorKind, Primitive, Shape, ShapeSummary,
    chamfer, fillet, make, make_box, summarize,
};
use silico_protocol::{Generation, Op, Reply};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TIMEOUT: Duration = Duration::from_secs(30);

fn box_10_20_30() -> Shape {
    make_box(10.0, 20.0, 30.0).unwrap()
}

fn cylinder() -> Shape {
    make(&Primitive::Cylinder {
        radius: 5.0,
        height: 10.0,
    })
    .unwrap()
}

/// Asserts a chamfer of the 10 × 20 × 30 box's Edge1 cut where the geometry says. Edge1
/// runs along z from (0, 0, 30) to (0, 0, 0), between Face1 (x = 0) and Face3 (y = 0). A
/// cut reaching `along_face1` along Face1 (in y) and `along_face3` along Face3 (in x)
/// removes a prism 30 long whose section is the right triangle with those legs: Face1 and
/// Face3 each lose a strip that wide, each end face loses the triangle, and the chamfer
/// face is its hypotenuse times 30. The prism's center is at a third of each leg.
fn assert_edge1_cut(case: &str, s: &ShapeSummary, along_face1: f64, along_face3: f64) {
    let (a, b) = (along_face1, along_face3);
    let removed = 15.0 * a * b;
    let volume = 6000.0 - removed;
    let area = 2200.0 - 30.0 * a - 30.0 * b - a * b + 30.0 * (a * a + b * b).sqrt();
    let center = [
        (6000.0 * 5.0 - removed * b / 3.0) / volume,
        (6000.0 * 10.0 - removed * a / 3.0) / volume,
        15.0,
    ];
    assert_close(&format!("{case}: volume"), s.volume, volume, 1e-12);
    assert_close(&format!("{case}: area"), s.area, area, 1e-12);
    for (axis, (got, want)) in s.center.iter().zip(center).enumerate() {
        assert_close(&format!("{case}: center {axis}"), *got, want, 1e-12);
    }
    assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10), "{case}");
    assert!(s.valid, "{case}");
}

#[test]
fn chamfer_edge() {
    // The roadmap's: d = 2 on one 30-long edge removes 2 · 2 / 2 · 30 = 60.
    let cut = chamfer(&box_10_20_30(), &[1], equal(2.0), false).unwrap();
    let s = summarize(&cut.shape).unwrap();
    assert_close("volume", s.volume, 5940.0, 1e-12);
    assert_edge1_cut("d = 2", &s, 2.0, 2.0);
}

#[test]
fn each_kind_cuts_where_the_geometry_says() {
    // tan 30° = 1/√3 and tan 60° = √3, exactly.
    let root3 = 3f64.sqrt();
    // (size, flip, reach along Face1, reach along Face3). Face1 is Edge1's lower-numbered
    // face, so `distance` goes along it, and along Face3 when flipped (FreeCAD 1.1.3 cuts
    // the same; docs/ports/chamfer.md). An angle is from the face `distance` is on, so the
    // cut reaches distance · tan(angle) along the other.
    let cases = [
        (equal(2.0), true, 2.0, 2.0),
        (two(1.0, 3.0), false, 1.0, 3.0),
        (two(1.0, 3.0), true, 3.0, 1.0),
        (angled(1.0, 30.0), false, 1.0, 1.0 / root3),
        (angled(1.0, 30.0), true, 1.0 / root3, 1.0),
        (angled(2.0, 60.0), false, 2.0, 2.0 * root3),
        (angled(1.0, 45.0), false, 1.0, 1.0),
    ];
    for (size, flip, a, b) in cases {
        let case = format!("{size:?} flip {flip}");
        let cut = chamfer(&box_10_20_30(), &[1], size, flip).unwrap();
        assert_edge1_cut(&case, &summarize(&cut.shape).unwrap(), a, b);
    }
}

#[test]
fn two_edges_meeting_at_a_corner_match_freecads_formula() {
    // FreeCAD's FeatureChamfer testOther: two edges of lengths L and H meeting at a corner,
    // each chamfered by c, remove L c²/2 + H c²/2 − c³/3. Edge1 (along z, 30 long) and
    // Edge2 (along y, 20 long) meet at (0, 0, 30).
    for c in [0.5, 1.0, 4.0] {
        let cut = chamfer(&box_10_20_30(), &[1, 2], equal(c), false).unwrap();
        let s = summarize(&cut.shape).unwrap();
        let removed = 30.0 * c * c / 2.0 + 20.0 * c * c / 2.0 - c * c * c / 3.0;
        assert_close(&format!("c = {c}"), s.volume, 6000.0 - removed, 1e-12);
        assert!(s.valid, "c = {c}");
    }
}

/// Every edge of a cube of side `side` chamfered by `c`: each face shrinks to a square of
/// side − 2c, each edge leaves a strip c√2 wide and side − 2c long, and each corner a
/// triangle with sides c√2. Each edge removes a prism of section c²/2, and each corner
/// 5c³/6 more (its c³ corner cube less the c³/6 tetrahedron the three cuts leave).
fn cube_every_edge(side: f64, c: f64) -> (f64, f64) {
    let inner = side - 2.0 * c;
    let volume = side * side * side - 12.0 * (c * c / 2.0) * inner - 8.0 * (5.0 / 6.0) * c * c * c;
    let area = 6.0 * inner * inner
        + 12.0 * (c * 2f64.sqrt()) * inner
        + 8.0 * (3f64.sqrt() / 4.0) * 2.0 * c * c;
    (volume, area)
}

#[test]
fn freecad_chamfer_every_edge_of_a_unit_cube() {
    // TopoShapeExpansion.cpp's makeElementChamfer: every edge at 0.05, area 5.640996.
    let all: Vec<u32> = (1..=12).collect();
    let cube = make_box(1.0, 1.0, 1.0).unwrap();
    let s = summarize(&chamfer(&cube, &all, equal(0.05), false).unwrap().shape).unwrap();
    assert!((s.area - 5.640996).abs() <= 1e-6, "{}", s.area);
    let (volume, area) = cube_every_edge(1.0, 0.05);
    assert_close("volume", s.volume, volume, 1e-12);
    assert_close("area", s.area, area, 1e-12);
    assert_eq!(s.faces, 26);
    assert!(s.valid);
}

/// The areas of a shape's faces, from its mesh, which is exact for flat faces.
fn face_areas(shape: &Shape) -> Vec<f64> {
    let mesh = silico_kernel::tessellate(shape, 0.01).unwrap();
    let mut areas = Vec::new();
    let mut triangles = mesh.triangles.iter();
    for &count in &mesh.face_triangles {
        let mut area = 0.0;
        for t in triangles.by_ref().take(count as usize) {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            let (u, v) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            area += (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0;
        }
        areas.push(area);
    }
    areas
}

#[test]
fn freecad_chamfer_cube_to_octahedron() {
    // TestChamfer.py's testChamferCubeToOctahedron: every edge of a 10 cube at 4.999999
    // leaves 8 faces larger than 1e-3 (nearly the octahedron through the faces' centers),
    // and the four edges of Face1 leave 9.
    let cube = make_box(10.0, 10.0, 10.0).unwrap();
    let c = 4.999999;
    let all: Vec<u32> = (1..=12).collect();
    let octahedron = chamfer(&cube, &all, equal(c), false).unwrap().shape;
    let s = summarize(&octahedron).unwrap();
    let (volume, area) = cube_every_edge(10.0, c);
    assert_close("volume", s.volume, volume, 1e-9);
    assert_close("area", s.area, area, 1e-9);
    assert!(s.valid);
    let major = |shape: &Shape| face_areas(shape).iter().filter(|&&a| a > 1e-3).count();
    assert_eq!(major(&octahedron), 8);

    // Face1's edges are Edge1 to Edge4: around the face, each pair meeting at a corner,
    // so four prisms 10 long, less the four overlaps (testOther's c³/3).
    let pyramid = chamfer(&cube, &[1, 2, 3, 4], equal(c), false)
        .unwrap()
        .shape;
    let s = summarize(&pyramid).unwrap();
    let removed = 4.0 * 10.0 * c * c / 2.0 - 4.0 * c * c * c / 3.0;
    assert_close("volume", s.volume, 1000.0 - removed, 1e-9);
    assert!(s.valid);
    assert_eq!(major(&pyramid), 9);
}

#[test]
fn a_cylinders_rim_is_cut_into_a_cone() {
    // The cut on a rim of the r = 5, h = 10 cylinder is a right triangle with legs a
    // (along the end face) and b (along the side), revolved. By Pappus it removes 2π ∫ r dA
    // and moves the center by its moment 2π ∫ r z dA, each exact here by the midpoint rule
    // (exact for quadratics). The cylinder's side is its Face1, so `distance` goes along it
    // (b), and along the end face when flipped. (Edge1 is the seam, Edge2 the bottom rim,
    // Edge3 the top.)
    let check = |edge: u32, size: ChamferSize, flip: bool, a: f64, b: f64| {
        let case = format!("Edge{edge} {size:?} flip {flip}");
        let s = summarize(&chamfer(&cylinder(), &[edge], size, flip).unwrap().shape).unwrap();
        let (end, inward) = if edge == 2 { (0.0, 1.0) } else { (10.0, -1.0) };
        let corners = [(5.0, end), (5.0 - a, end), (5.0, end + inward * b)];
        let mid = |i: usize, j: usize| {
            let (p, q): ((f64, f64), (f64, f64)) = (corners[i], corners[j]);
            ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0)
        };
        let mids = [mid(0, 1), mid(1, 2), mid(2, 0)];
        let triangle = a * b / 2.0;
        let ring = 2.0 * PI * triangle / 3.0 * mids.iter().map(|m| m.0).sum::<f64>();
        let moment = 2.0 * PI * triangle / 3.0 * mids.iter().map(|m| m.0 * m.1).sum::<f64>();
        let volume = 250.0 * PI - ring;
        // The side loses a band b high, the end an annulus a wide; the cone's side has
        // slant √(a² + b²) between radii 5 and 5 − a.
        let area = 150.0 * PI - 10.0 * PI * b - PI * (25.0 - (5.0 - a) * (5.0 - a))
            + PI * (5.0 + 5.0 - a) * (a * a + b * b).sqrt();
        assert_close(&format!("{case}: volume"), s.volume, volume, 1e-9);
        assert_close(&format!("{case}: area"), s.area, area, 1e-9);
        assert_close(
            &format!("{case}: center"),
            s.center[2],
            (250.0 * PI * 5.0 - moment) / volume,
            1e-9,
        );
        assert_eq!((s.faces, s.valid), (4, true), "{case}");
    };
    check(2, equal(1.0), false, 1.0, 1.0);
    check(3, equal(1.0), false, 1.0, 1.0);
    check(2, two(1.0, 2.0), false, 2.0, 1.0);
    check(2, two(1.0, 2.0), true, 1.0, 2.0);
    check(3, two(0.5, 3.0), false, 3.0, 0.5);
}

/// FreeCAD's PartDesign Chamfer on a body based on a Part primitive, for each case:
/// volume, area, center, counts, validity, or that it failed.
fn freecad_chamfers(cases: &[Case]) -> Vec<serde_json::Value> {
    let specs: Vec<String> = cases
        .iter()
        .map(|c| {
            let (kind, props) = match c.base {
                Base::Box => ("Box", r#"{"Length": 10.0, "Width": 20.0, "Height": 30.0}"#),
                Base::Cylinder => ("Cylinder", r#"{"Radius": 5.0, "Height": 10.0}"#),
                Base::Cone => (
                    "Cone",
                    r#"{"Radius1": 5.0, "Radius2": 2.0, "Height": 10.0}"#,
                ),
            };
            let (typ, size, size2, angle) = match c.size {
                ChamferSize::Equal { distance } => ("Equal distance", distance, 1.0, 45.0),
                ChamferSize::TwoDistances { distance, other } => {
                    ("Two distances", distance, other, 45.0)
                }
                ChamferSize::DistanceAngle { distance, angle } => {
                    ("Distance and Angle", distance, 1.0, angle)
                }
            };
            let edges: Vec<String> = c.edges.iter().map(|e| format!("'Edge{e}'")).collect();
            format!(
                "(\"{kind}\", {props}, [{}], \"{typ}\", {size:?}, {size2:?}, {angle:?}, {})",
                edges.join(", "),
                if c.flip { "True" } else { "False" }
            )
        })
        .collect();
    let script = format!(
        r#"
import json, FreeCAD
out = []
for kind, props, edges, typ, size, size2, angle, flip in [{}]:
    doc = FreeCAD.newDocument()
    prim = doc.addObject("Part::" + kind, kind)
    for k, v in props.items():
        setattr(prim, k, v)
    body = doc.addObject("PartDesign::Body", "Body")
    base = body.newObject("PartDesign::FeatureBase", "Base")
    base.BaseFeature = prim
    ch = body.newObject("PartDesign::Chamfer", "Chamfer")
    ch.Refine = False
    ch.Base = (base, edges)
    ch.ChamferType = typ
    ch.Size = size
    ch.Size2 = size2
    ch.Angle = angle
    ch.FlipDirection = flip
    doc.recompute()
    s = ch.Shape
    if s.isNull() or not ch.isValid():
        out.append({{"failed": ch.getStatusString()}})
    else:
        c = s.Solids[0].CenterOfMass
        out.append({{"volume": s.Volume, "area": s.Area, "center": [c.x, c.y, c.z],
                    "faces": len(s.Faces), "edges": len(s.Edges), "vertices": len(s.Vertexes),
                    "valid": s.isValid()}})
    FreeCAD.closeDocument(doc.Name)
print("SILICO_ORACLE " + json.dumps(out))
"#,
        specs.join(", ")
    );
    oracle::freecad(&script).as_array().unwrap().clone()
}

#[derive(Debug, Clone, Copy)]
enum Base {
    Box,
    Cylinder,
    Cone,
}

#[derive(Debug, Clone, Copy)]
struct Case {
    base: Base,
    edges: &'static [u32],
    size: ChamferSize,
    flip: bool,
}

const fn case(base: Base, edges: &'static [u32], size: ChamferSize, flip: bool) -> Case {
    Case {
        base,
        edges,
        size,
        flip,
    }
}

const ALL_EDGES: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

/// Cases FreeCAD and Silico both run, each kind with and without the flip, edges that
/// meet, a rim, and sizes either side of failing.
const CASES: &[Case] = &[
    case(Base::Box, &[1], ChamferSize::Equal { distance: 2.0 }, false),
    case(Base::Box, &[1], ChamferSize::Equal { distance: 2.0 }, true),
    case(Base::Box, &[3], ChamferSize::Equal { distance: 2.0 }, false),
    case(
        Base::Box,
        &[1, 2],
        ChamferSize::Equal { distance: 1.0 },
        false,
    ),
    case(
        Base::Box,
        ALL_EDGES,
        ChamferSize::Equal { distance: 1.0 },
        false,
    ),
    case(
        Base::Box,
        ALL_EDGES,
        ChamferSize::Equal { distance: 4.99 },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::TwoDistances {
            distance: 1.0,
            other: 3.0,
        },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::TwoDistances {
            distance: 1.0,
            other: 3.0,
        },
        true,
    ),
    case(
        Base::Box,
        &[1, 2],
        ChamferSize::TwoDistances {
            distance: 1.0,
            other: 3.0,
        },
        false,
    ),
    case(
        Base::Box,
        &[1, 2, 9],
        ChamferSize::TwoDistances {
            distance: 0.5,
            other: 2.0,
        },
        true,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::DistanceAngle {
            distance: 1.0,
            angle: 30.0,
        },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::DistanceAngle {
            distance: 1.0,
            angle: 30.0,
        },
        true,
    ),
    case(
        Base::Box,
        &[9],
        ChamferSize::DistanceAngle {
            distance: 2.0,
            angle: 60.0,
        },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::Equal { distance: 9.999 },
        false,
    ),
    case(
        Base::Cylinder,
        &[2],
        ChamferSize::Equal { distance: 1.0 },
        false,
    ),
    case(
        Base::Cylinder,
        &[3],
        ChamferSize::TwoDistances {
            distance: 1.0,
            other: 2.0,
        },
        true,
    ),
    case(
        Base::Cone,
        &[1, 3],
        ChamferSize::Equal { distance: 0.5 },
        false,
    ),
    case(
        Base::Cone,
        &[3],
        ChamferSize::DistanceAngle {
            distance: 1.0,
            angle: 40.0,
        },
        true,
    ),
    // Too large for the edge: both fail.
    case(
        Base::Box,
        &[1],
        ChamferSize::Equal { distance: 10.0 },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::TwoDistances {
            distance: 1.0,
            other: 10.0,
        },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::DistanceAngle {
            distance: 1.0,
            angle: 89.0,
        },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::DistanceAngle {
            distance: 1.0,
            angle: 90.0,
        },
        false,
    ),
    case(
        Base::Box,
        &[1],
        ChamferSize::DistanceAngle {
            distance: 1.0,
            angle: 135.0,
        },
        false,
    ),
];

fn base_shape(base: Base) -> Shape {
    match base {
        Base::Box => box_10_20_30(),
        Base::Cylinder => cylinder(),
        Base::Cone => make(&Primitive::Cone {
            radius1: 5.0,
            radius2: 2.0,
            height: 10.0,
        })
        .unwrap(),
    }
}

#[test]
fn chamfers_match_freecad() {
    let theirs = freecad_chamfers(CASES);
    assert_eq!(theirs.len(), CASES.len());
    let mut failed = 0;
    for (c, theirs) in CASES.iter().zip(&theirs) {
        let case = format!("{c:?}");
        let ours = chamfer(&base_shape(c.base), c.edges, c.size, c.flip);
        if theirs.get("failed").is_some() {
            let err = ours
                .err()
                .unwrap_or_else(|| panic!("{case}: FreeCAD failed, Silico didn't"));
            assert_eq!(err.kind(), KernelErrorKind::Failed, "{case}: {err}");
            failed += 1;
            continue;
        }
        let s = summarize(&ours.unwrap_or_else(|e| panic!("{case}: {e}")).shape).unwrap();
        // FreeCAD measures with OCCT's fixed-order integration, Silico adaptively.
        let tolerance = if matches!(c.base, Base::Box) {
            1e-12
        } else {
            1e-8
        };
        assert_close(
            &case,
            s.volume,
            theirs["volume"].as_f64().unwrap(),
            tolerance,
        );
        assert_close(&case, s.area, theirs["area"].as_f64().unwrap(), tolerance);
        for (axis, got) in s.center.iter().enumerate() {
            let want = theirs["center"][axis].as_f64().unwrap();
            assert!(
                (got - want).abs() <= 1e-8,
                "{case}: center {axis}: {got} vs {want}"
            );
        }
        let count = |key: &str| theirs[key].as_u64().unwrap() as u32;
        assert_eq!(
            (s.faces, s.edges, s.vertices),
            (count("faces"), count("edges"), count("vertices")),
            "{case}"
        );
        assert_eq!(s.valid, theirs["valid"].as_bool().unwrap(), "{case}");
    }
    assert_eq!(failed, 5, "the cases too large for their edge");
}

/// The facts FreeCAD's element names state for `Part.Shape.makeChamfer(distance, other,
/// edges)` on the 10 × 20 × 30 box (docs/ports/history.md; the parsing is the boolean
/// test's, for one input). A name the script can't read is returned, so it fails the test.
fn freecad_chamfer_facts(edges: &[u32], distance: f64, other: f64) -> (Vec<Fact>, Vec<String>) {
    let edges: Vec<String> = edges
        .iter()
        .map(|e| format!("a.Shape.Edges[{}]", e - 1))
        .collect();
    let script = format!(
        r#"
import json, re, FreeCAD
doc = FreeCAD.newDocument()
a = doc.addObject("Part::Box", "Box"); a.Length = 10.0; a.Width = 20.0; a.Height = 30.0
doc.recompute()
r = a.Shape.makeChamfer({distance:?}, {other:?}, [{}])
EL = r"([A-Za-z]+[0-9]+)"
kept = re.compile(EL + r";:H([0-9a-f]+),[FEV]")
made = re.compile(EL + r";:([MG])([0-9]*);CHF;:H([0-9a-f]+):[0-9a-f]+,[FEV]")
facts, unread = [], []
for result, name in r.ElementReverseMap.items():
    if ";:U" in name or ";:L" in name:
        continue
    m = kept.fullmatch(name)
    if m:
        facts.append(["kept", m[1], result, 0])
        continue
    m = made.fullmatch(name)
    if not m:
        unread.append(name)
        continue
    facts.append(["modified" if m[2] == "M" else "generated", m[1], result, int(m[3] or 1)])
print("SILICO_ORACLE " + json.dumps({{"facts": facts, "unread": unread}}))
"#,
        edges.join(", ")
    );
    let out = oracle::freecad(&script);
    let mut stated: Vec<Fact> = out["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let s = |i: usize| f[i].as_str().unwrap().to_owned();
            (s(0), s(1), s(2), f[3].as_u64().unwrap() as u32)
        })
        .collect();
    stated.sort();
    let unread = out["unread"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    (stated, unread)
}

#[test]
fn chamfer_history_matches_freecads_element_names() {
    let cases: [(&[u32], f64, f64); 4] = [
        (&[1], 2.0, 2.0),
        (&[1], 1.0, 3.0),
        (&[1, 2], 1.0, 1.5),
        (ALL_EDGES, 1.0, 1.0),
    ];
    for (edges, distance, other) in cases {
        let case = format!("{edges:?} at {distance}, {other}");
        let (stated, unread) = freecad_chamfer_facts(edges, distance, other);
        assert_eq!(unread, Vec::<String>::new(), "{case}");
        assert!(stated.len() > 3, "{case}: FreeCAD stated only {stated:?}");
        let built = chamfer(&box_10_20_30(), edges, two(distance, other), false).unwrap();
        let ours = facts(&built.history);
        for fact in &stated {
            assert!(
                ours.contains(fact),
                "{case}: FreeCAD has {fact:?}; Silico has {ours:?}"
            );
        }
        let kept =
            |fs: &[Fact]| -> Vec<Fact> { fs.iter().filter(|f| f.0 == "kept").cloned().collect() };
        assert_eq!(kept(&ours), kept(&stated), "{case}: kept");
    }
}

#[test]
fn chamfering_one_edge_keeps_modifies_and_generates_what_the_geometry_says() {
    // As for the fillet (history.rs): cutting Edge1 trims the two faces it joins (Face1,
    // Face3) and the two end caps (Face5, Face6), leaves Face2 and Face4, and makes one new
    // face, from Edge1, which is gone. The edges meeting Edge1's ends (2, 4, 9, 10) are
    // shortened, and Edge1's two end vertices go.
    let h = chamfer(&box_10_20_30(), &[1], two(1.0, 3.0), false)
        .unwrap()
        .history;
    assert_eq!(h.validate(1), Ok(()));
    let input = &h.inputs[0];
    let kept: Vec<bool> = input.faces.iter().map(|f| f.kept.is_some()).collect();
    assert_eq!(kept, [false, true, false, true, false, false], "faces kept");
    for (i, face) in input.faces.iter().enumerate() {
        if face.kept.is_none() {
            assert_eq!(face.modified.len(), 1, "Face{}", i + 1);
        }
    }
    let edge1 = &input.edges[0];
    assert!(edge1.is_deleted(), "{edge1:?}");
    assert_eq!(edge1.generated.len(), 1, "{edge1:?}");
    assert_eq!(edge1.generated[0].element.kind, ElementKind::Face);
    let generators: Vec<Fact> = facts(&h)
        .into_iter()
        .filter(|f| f.0 == "generated")
        .collect();
    assert_eq!(generators.len(), 1, "{generators:?}");
    for (i, edge) in input.edges.iter().enumerate().skip(1) {
        let shortened = [2, 4, 9, 10].contains(&(i + 1));
        assert_eq!(edge.kept.is_none(), shortened, "Edge{}: {edge:?}", i + 1);
    }
    assert_eq!(input.vertices.iter().filter(|v| v.is_deleted()).count(), 2);
}

/// Two 10 × 20 × 30 boxes fused along the edge they share (x = 10, y = 20): that edge,
/// Edge11, joins four faces, and Edges 8, 9, 10, 12, 13, 15, 16, and 18 end on it.
fn boxes_sharing_an_edge() -> Shape {
    let b = box_10_20_30();
    let moved = silico_kernel::translate(&b, [10.0, 20.0, 0.0])
        .unwrap()
        .shape;
    silico_kernel::boolean(silico_kernel::BooleanOp::Fuse, &b, &moved)
        .unwrap()
        .shape
}

/// A filleted box (Edge1 at r = 2): its Edge2 and Edge10 run where the fillet meets the
/// faces it joins, which meet it smoothly.
fn filleted_box() -> Shape {
    fillet(&box_10_20_30(), &[1], 2.0).unwrap().shape
}

#[test]
fn an_edge_occt_wont_take_is_refused_naming_it() {
    let sphere = make(&Primitive::Sphere { radius: 5.0 }).unwrap();
    let cone = make(&Primitive::Cone {
        radius1: 5.0,
        radius2: 0.0,
        height: 10.0,
    })
    .unwrap();
    let open = silico_kernel::test_ops::open_box().unwrap();
    let loose = silico_kernel::test_ops::box_with_loose_edge().unwrap();
    // (shape, edges, the edge named, why). OCCT throws "There are no suitable edges" for
    // most of these alone, skips them silently among others, and segfaults on Edge11 of
    // the fused boxes (so that one is never given to it).
    let cases: [(&str, &Shape, &[u32], &str, &str); 9] = [
        (
            "cylinder",
            &cylinder(),
            &[1],
            "Edge1",
            "less than 0.1 radian",
        ),
        (
            "cylinder",
            &cylinder(),
            &[2, 1],
            "Edge1",
            "less than 0.1 radian",
        ),
        ("sphere", &sphere, &[1], "Edge1", "is degenerate"),
        ("cone", &cone, &[3, 1], "Edge1", "is degenerate"),
        ("open box", &open, &[1, 7], "Edge7", "only one face"),
        ("loose edge", &loose, &[13], "Edge13", "is on no face"),
        (
            "boxes sharing an edge",
            &boxes_sharing_an_edge(),
            &[1, 11],
            "Edge11",
            "more than two faces",
        ),
        (
            "filleted box",
            &filleted_box(),
            &[2],
            "Edge2",
            "less than 0.1 radian",
        ),
        (
            "filleted box",
            &filleted_box(),
            &[3, 2],
            "Edge2",
            "less than 0.1 radian",
        ),
    ];
    for (name, shape, edges, element, why) in cases {
        let results: [(&str, Result<Built, KernelError>); 2] = [
            ("chamfer", chamfer(shape, edges, equal(0.5), false)),
            ("fillet", fillet(shape, edges, 0.5)),
        ];
        for (operation, result) in results {
            let case = format!("{operation} of {name} {edges:?}");
            let Err(err) = result else {
                panic!("{case}: built");
            };
            assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{case}: {err}");
            assert_eq!(err.element(), Some(element), "{case}: {err}");
            assert!(err.message().starts_with(element), "{case}: {err}");
            assert!(err.message().contains(why), "{case}: {err}");
            assert!(err.message().contains(operation), "{case}: {err}");
            assert_eq!(
                err.suggestion(),
                Some(format!("leave {element} out of the {operation}").as_str()),
                "{case}"
            );
        }
    }
    // Every edge OCCT won't take is named, the lowest-numbered as the element, whatever
    // order they're listed in.
    let err = chamfer(&filleted_box(), &[10, 3, 2], equal(0.5), false)
        .err()
        .unwrap();
    assert_eq!(err.element(), Some("Edge2"), "{err}");
    assert!(
        err.message()
            .ends_with("; OCCT won't chamfer Edge10 either"),
        "{err}"
    );
    // The open box's input is invalid, so the chamfer of an edge OCCT does take isn't
    // refused for that (docs/ports/fillet.md): its result is returned, invalid.
    let s = summarize(&chamfer(&open, &[1], equal(0.5), false).unwrap().shape).unwrap();
    assert!(!s.valid);
}

#[test]
fn an_edge_ending_where_solids_touch_fails_saying_why() {
    // OCCT takes Edge8 (it has two faces), but fails at its end, on Edge11: "only 2
    // faces". The size has nothing to do with it, and the suggestion says so.
    let fused = boxes_sharing_an_edge();
    for (operation, result) in [
        ("chamfer", chamfer(&fused, &[8], equal(0.5), false)),
        ("fillet", fillet(&fused, &[8], 0.5)),
    ] {
        let err = result.err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::Failed, "{operation}: {err}");
        assert!(err.message().contains("only 2 faces"), "{operation}: {err}");
        assert_eq!(err.element(), Some("Edge8"));
        assert!(
            err.suggestion()
                .unwrap()
                .contains("more than two faces meet"),
            "{operation}: {err}"
        );
    }
    // OCCT throws taking Edge8, so it's Edge8 that's blamed among several.
    let err = chamfer(&fused, &[1, 8], equal(0.5), false).err().unwrap();
    assert_eq!(err.kind(), KernelErrorKind::Failed, "{err}");
    assert_eq!(err.element(), Some("Edge8"), "{err}");
    assert!(
        err.suggestion()
            .unwrap()
            .contains("more than two faces meet"),
        "{err}"
    );
    // An edge OCCT mustn't be given is refused before any is given: with Edge8, Edge11
    // (joining four faces) is refused, named, rather than lost behind OCCT's failure on
    // Edge8.
    let err = chamfer(&fused, &[8, 11], equal(0.5), false).err().unwrap();
    assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{err}");
    assert_eq!(err.element(), Some("Edge11"), "{err}");
    assert_eq!(err.suggestion(), Some("leave Edge11 out of the chamfer"));
    // The edges away from Edge11 chamfer.
    assert!(chamfer(&fused, &[1, 14], equal(0.5), false).is_ok());
}

#[test]
fn a_chamfer_too_large_names_the_edge_and_what_to_try() {
    let b = box_10_20_30();
    // OCCT doesn't say why it failed: too large for a face beside the edge, as here, or
    // (on curved faces) too small, or at any size. The suggestion offers each.
    let cases = [
        (equal(10.0), "a smaller size", "another size"),
        (two(1.0, 10.0), "smaller distances", "other distances"),
        (
            angled(1.0, 89.0),
            "a smaller distance or angle",
            "another distance or angle",
        ),
    ];
    for (size, smaller, other) in cases {
        let err = chamfer(&b, &[3], size, false).err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::Failed, "{size:?}: {err}");
        assert_eq!(
            err.message(),
            "Standard_Failure: the chamfer couldn't be built",
            "{size:?}"
        );
        assert_eq!(err.element(), Some("Edge3"), "{size:?}");
        let suggestion = format!(
            "if the chamfer runs past a face beside the edge, try {smaller}; if not, try {other}, or leave the edge out: OCCT can't chamfer some edges at any size"
        );
        assert_eq!(err.suggestion(), Some(suggestion.as_str()), "{size:?}");
    }
    // A fillet's failure says it's the fillet's.
    let err = fillet(&b, &[3], 11.0).err().unwrap();
    assert_eq!(
        err.message(),
        "Standard_Failure: the fillet couldn't be built"
    );
    let err = chamfer(&b, &[1, 3], equal(10.0), false).err().unwrap();
    assert_eq!(err.element(), None, "{err}");
    assert_eq!(
        err.suggestion(),
        Some(
            "chamfer the edges one at a time to find the one that fails; then, if its chamfer runs past a face beside it, try a smaller size, and if not, another size, or leave it out"
        )
    );
}

#[test]
fn an_invalid_chamfer_result_is_refused() {
    // FreeCAD's FeatureChamfer testOther, unrefined: a 4 × 5 × 6 box with a 1 × 2 × 3 box
    // on top at (0, 1, 6), fused. Edge1 runs from (0, 0, 6) to (0, 0, 0), Edge2 from
    // (0, 0, 6) to (0, 1, 6). Chamfering them at 0.5 gives a "solid" that isn't valid,
    // which FreeCAD 1.1.3 returns (volume 125.17, more than the 124.79 the cut leaves when
    // refined); Silico refuses it.
    let big = make_box(4.0, 5.0, 6.0).unwrap();
    let small = make_box(1.0, 2.0, 3.0).unwrap();
    let small = silico_kernel::translate(&small, [0.0, 1.0, 6.0])
        .unwrap()
        .shape;
    let fused = silico_kernel::boolean(silico_kernel::BooleanOp::Fuse, &big, &small)
        .unwrap()
        .shape;
    assert!(summarize(&fused).unwrap().valid);
    // Edge2 alone breaks it too (FreeCAD: valid False, volume 125.875); Edge1 alone doesn't.
    for edges in [&[1, 2][..], &[2]] {
        let err = chamfer(&fused, edges, equal(0.5), false).err().unwrap();
        assert_eq!(
            err.kind(),
            KernelErrorKind::InvalidResult,
            "{edges:?}: {err}"
        );
        assert_eq!(err.message(), "the chamfer's result isn't a valid solid");
    }
    let s = summarize(&chamfer(&fused, &[1], equal(0.5), false).unwrap().shape).unwrap();
    // Edge1 is 6 long: 6 · 0.5² / 2 = 0.75 of the 126.
    assert_close("volume", s.volume, 126.0 - 0.75, 1e-12);
    assert!(s.valid);
}

#[test]
fn a_size_that_cant_work_never_reaches_the_worker() {
    let mut k = kernel();
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let sent = k.requests_sent();
    for (size, parameter) in [
        (equal(0.0), "distance"),
        (equal(1e-7), "distance"),
        (equal(f64::NAN), "distance"),
        (equal(2e6), "distance"),
        (two(1.0, -1.0), "other distance"),
        (two(f64::INFINITY, 1.0), "distance"),
        (angled(1.0, 0.0), "angle"),
        (angled(1.0, 180.0), "angle"),
        (angled(1.0, f64::NAN), "angle"),
        (angled(-1.0, 45.0), "distance"),
        // A cut under the tolerance: 1 · 1e-6° in radians is 1.7e-8.
        (angled(1.0, 1e-6), "angle"),
        (angled(1e-3, 5e-3), "angle"),
    ] {
        let err = k.chamfer(&shape, &[1], size, false).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{size:?}: {err}");
        assert_eq!(
            err.suggestion(),
            Some(format!("give the chamfer a valid {parameter}").as_str()),
            "{size:?}"
        );
    }
    assert_eq!(k.requests_sent(), sent, "a bad size was sent");
}

#[test]
fn the_worker_refuses_what_the_core_would_and_goes_on() {
    let mut w = Worker::spawn(Path::new(WORKER)).unwrap();
    let Reply::Shape(shape) = w
        .call(
            Generation(1),
            Op::MakePrimitive(Primitive::Box {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            }),
            TIMEOUT,
        )
        .unwrap()
    else {
        panic!("not a shape");
    };
    let mut call = |edges: &[u32], size: ChamferSize| {
        w.call(
            Generation(1),
            Op::Chamfer {
                shape: shape.clone(),
                edges: edges.to_vec(),
                size,
                flip: false,
            },
            TIMEOUT,
        )
    };
    for (edges, size, says, element) in [
        (&[1][..], equal(-1.0), "distance", None),
        (&[1], angled(1.0, 200.0), "angle", None),
        (&[], equal(1.0), "a chamfer needs at least one edge", None),
        (&[0], equal(1.0), "Edge0 doesn't exist", Some("Edge0")),
        (
            &[13],
            equal(1.0),
            "the shape has edges 1 to 12",
            Some("Edge13"),
        ),
        (&[2, 2], equal(1.0), "Edge2 is listed twice", Some("Edge2")),
    ] {
        let err = call(edges, size).unwrap_err();
        assert_eq!(
            err.code(),
            ErrorCode::InvalidInput,
            "{edges:?} {size:?}: {err}"
        );
        assert!(err.message().contains(says), "{edges:?} {size:?}: {err}");
        assert_eq!(err.element(), element, "{edges:?} {size:?}: {err}");
    }
    let err = call(&[1], equal(10.0)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.element(), Some("Edge1"));
    // The worker goes on.
    assert!(matches!(call(&[1], equal(2.0)), Ok(Reply::Built(_))));
}

#[test]
fn a_chamfer_through_the_worker_is_the_kernels_and_the_same_every_time() {
    let mut k = kernel();
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let size = angled(1.5, 35.0);
    let first = k.chamfer(&shape, &[1, 2, 9], size, true).unwrap();
    let again = kernel().chamfer(&shape, &[1, 2, 9], size, true).unwrap();
    assert!(first.shape == again.shape, "different bytes");
    assert_eq!(first.history, again.history);
    let here = chamfer(&box_10_20_30(), &[1, 2, 9], size, true).unwrap();
    assert_eq!(first.history, here.history);
    let theirs = k.summarize(&first.shape).unwrap();
    let ours = summarize(&here.shape).unwrap();
    assert_close("volume", theirs.volume, ours.volume, 1e-12);
    assert_eq!(theirs.faces, ours.faces);
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
