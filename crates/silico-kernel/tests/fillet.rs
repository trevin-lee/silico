//! M0.8: the fillet, checked against analytic values in-process, and against FreeCAD
//! through the worker (the M0 exit test: a request crosses the process boundary, the cxx
//! bridge, and the deterministic OCCT build, and agrees with FreeCAD).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::imports;
use super::kernel;
use super::numbers;
use super::oracle;
use crate::wait::WAIT;

use kernel::kernel;
use numbers::relative_error;
use std::f64::consts::PI;
use std::path::Path;

use silico_core::ErrorCode;
use silico_core::kernel::KernelWorker;
use silico_core::worker::Worker;
use silico_protocol::{
    Blob, DebugOp, Generation, Misbehavior, Op, Primitive, Reply, ShapeData, ShapeSummary,
};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

#[test]
fn box_fillet_analytic() {
    // Edge 1 of a 10 × 20 × 30 box runs along z, length 30 (FreeCAD agrees; see below).
    let rounded = silico_kernel::fillet(
        &silico_kernel::make_box(10.0, 20.0, 30.0).unwrap(),
        &[1],
        2.0,
    )
    .unwrap()
    .shape;
    let s = silico_kernel::summarize(&rounded).unwrap();
    // Analytic: the fillet removes a corner of cross-section r² − πr²/4 = 4 − π along the
    // edge's 30; the area loses two 2 × 30 strips, gains a quarter cylinder 2πr·30/4, and
    // each end face loses 4 − π.
    let volume = 6000.0 - (4.0 - PI) * 30.0;
    let area = 2200.0 - 120.0 + 30.0 * PI - 2.0 * (4.0 - PI);
    assert!(
        relative_error(s.volume, volume) <= 1e-12,
        "volume {} vs {volume}",
        s.volume
    );
    assert!(
        relative_error(s.area, area) <= 1e-12,
        "area {} vs {area}",
        s.area
    );
    assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10));
    assert!(s.valid);
}

#[test]
fn an_invalid_shape_summarizes_as_invalid() {
    // A box with a face missing: its shell doesn't close.
    let open = silico_kernel::test_ops::open_box().unwrap();
    let s = silico_kernel::summarize(&open).unwrap();
    assert_eq!(s.faces, 5);
    assert!(!s.valid, "an open box was reported valid");
}

#[test]
fn an_unfilleted_box_summarizes_as_a_box() {
    let s = silico_kernel::summarize(&silico_kernel::make_box(10.0, 20.0, 30.0).unwrap()).unwrap();
    assert!(relative_error(s.volume, 6000.0) <= 1e-9);
    assert!(relative_error(s.area, 2200.0) <= 1e-9);
    assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8));
    assert!(s.valid);
}

fn worker() -> Worker {
    Worker::spawn(Path::new(WORKER)).unwrap()
}

/// A box, filleted, summarized: all through the worker.
fn fillet_through_worker(
    w: &mut Worker,
    edges: &[u32],
    radius: f64,
) -> Result<ShapeSummary, silico_core::Error> {
    let shape = match w.call(
        Generation(1),
        Op::MakePrimitive(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        }),
        WAIT,
    )? {
        Reply::Shape(shape) => shape,
        other => panic!("MakeBox replied {other:?}"),
    };
    let rounded = match w.call(
        Generation(1),
        Op::Fillet {
            shape,
            edges: edges.to_vec(),
            radius,
        },
        WAIT,
    )? {
        Reply::Built(built) => built.shape,
        other => panic!("Fillet replied {other:?}"),
    };
    match w.call(Generation(1), Op::Summarize { shape: rounded }, WAIT)? {
        Reply::Summary(summary) => Ok(summary),
        other => panic!("Summarize replied {other:?}"),
    }
}

/// The cases FreeCAD and Silico both run: (edges, radius).
const CASES: &[(&[u32], f64)] = &[
    (&[1], 2.0),
    (&[3], 2.0),
    (&[1, 3, 5, 7], 2.0),
    (&[1, 2], 1.5),
    (&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 1.0),
    (&[9], 4.0),
    (&[1], 9.99),
    (&[1], 11.0),
    (&[1], 10.0),
    // At or below OCCT's 1e-7 tolerance, and absurdly large: Silico refuses the radius as
    // it refuses a chamfer's distance (FreeCAD builds 1e-8, and refuses the others).
    (&[1], 1e-8),
    (&[1], 1e-12),
    (&[1], 1e300),
    // OCCT reports these done but the result isn't a valid solid: FreeCAD 1.1 returns it
    // anyway (a regression; see docs/ports/fillet.md), Silico refuses it.
    (&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 5.01),
    (&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 5.5),
];

/// FreeCAD's results for every case, from one `freecadcmd` run.
fn freecad_results() -> Vec<serde_json::Value> {
    let cases: Vec<String> = CASES
        .iter()
        .map(|(edges, r)| format!("({edges:?}, {r:?})"))
        .collect();
    let script = format!(
        r#"
import json, Part
box = Part.makeBox(10, 20, 30)
results = []
for edges, radius in [{cases}]:
    try:
        f = box.makeFillet(radius, [box.Edges[i - 1] for i in edges])
        # FreeCAD returns the result inside a compound; its one solid has the center.
        c = f.Solids[0].CenterOfMass if len(f.Solids) == 1 else f.BoundBox.Center
        results.append({{"volume": f.Volume, "area": f.Area, "center": [c.x, c.y, c.z],
                         "faces": len(f.Faces), "edges": len(f.Edges),
                         "vertices": len(f.Vertexes), "valid": f.isValid()}})
    except Exception as e:
        results.append({{"error": type(e).__name__ + ": " + str(e)}})
print("SILICO_ORACLE " + json.dumps(results))
"#,
        cases = cases.join(", ")
    );
    oracle::freecad(&script).as_array().unwrap().clone()
}

#[test]
fn box_fillet_through_worker_matches_freecad() {
    let mut w = worker();
    let freecad = freecad_results();
    let mut compared = 0;
    let mut refused_invalid = 0;
    let mut refused_radius = 0;
    for (&(edges, radius), theirs) in CASES.iter().zip(&freecad) {
        let ours = fillet_through_worker(&mut w, edges, radius);
        let case = format!("edges {edges:?}, radius {radius}");
        match (&ours, theirs.get("error")) {
            // A radius outside Silico's lengths: refused before OCCT sees it, whatever FreeCAD
            // makes of it.
            (Err(e), _) if !(radius > 1e-7 && radius <= 1e6) => {
                assert_eq!(e.code(), ErrorCode::InvalidInput, "{case}: {e}");
                refused_radius += 1;
            }
            (Ok(s), None) => {
                let t = |key: &str| theirs[key].as_f64().unwrap();
                assert!(
                    relative_error(s.volume, t("volume")) <= 1e-9,
                    "{case}: volume {} vs FreeCAD {}",
                    s.volume,
                    t("volume")
                );
                assert!(
                    relative_error(s.area, t("area")) <= 1e-9,
                    "{case}: area {} vs FreeCAD {}",
                    s.area,
                    t("area")
                );
                for (axis, ours) in s.center.iter().enumerate() {
                    let theirs = theirs["center"][axis].as_f64().unwrap();
                    assert!(
                        (ours - theirs).abs() <= 1e-9 * 30.0,
                        "{case}: center[{axis}] {ours} vs FreeCAD {theirs}"
                    );
                }
                let count = |key: &str| theirs[key].as_u64().unwrap();
                assert_eq!(
                    (
                        u64::from(s.faces),
                        u64::from(s.edges),
                        u64::from(s.vertices)
                    ),
                    (count("faces"), count("edges"), count("vertices")),
                    "{case}: topology"
                );
                assert!(
                    s.valid && theirs["valid"].as_bool().unwrap(),
                    "{case}: validity"
                );
                compared += 1;
            }
            // Both refuse it: the same spec.
            (Err(e), Some(_)) => assert_eq!(e.code(), ErrorCode::KernelFailed, "{case}: {e}"),
            // FreeCAD returns an invalid solid; Silico refuses it. Anything else differing
            // is a failure.
            (Err(e), None) if theirs["valid"] == false => {
                assert_eq!(e.code(), ErrorCode::InvalidResult, "{case}: {e}");
                assert_eq!(e.message(), INVALID_RESULT, "{case}: {e}");
                refused_invalid += 1;
            }
            (ours, theirs) => panic!("{case}: Silico gave {ours:?}, FreeCAD gave {theirs:?}"),
        }
    }
    // The comparison isn't vacuous: most cases succeed on both sides, and FreeCAD's
    // invalid results really are refused.
    assert_eq!(compared, 7, "cases compared");
    assert_eq!(refused_invalid, 2);
    assert_eq!(refused_radius, 3);
}

/// The message for a fillet OCCT built, but whose result was refused as invalid.
const INVALID_RESULT: &str = "Fillet gave an invalid result, which was refused";

#[test]
fn fillet_too_large_is_structured_error() {
    // r = 11 on an edge whose adjacent 10-wide face can't hold it. FreeCAD refuses it too
    // (box_fillet_through_worker_matches_freecad).
    let mut w = worker();
    let err = fillet_through_worker(&mut w, &[1], 11.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.message(), "Fillet failed");
    assert_eq!(err.elements(), ["Edge1"]);
    assert!(
        err.cause().unwrap().contains("StdFail_NotDone")
            || err.cause().unwrap().contains("couldn't be built"),
        "{err}"
    );
    assert_eq!(
        err.suggestion(),
        Some(
            "if the fillet runs past a face beside the edge, try a smaller radius; if not, try another radius, or leave the edge out: OCCT can't fillet some edges at any size"
        )
    );
    // The worker carries on.
    assert!(fillet_through_worker(&mut w, &[1], 2.0).is_ok());
}

#[test]
fn bad_fillet_requests_are_invalid_input() {
    let mut w = worker();
    // The element is named only when an edge of the shape is at fault, not for a bad
    // radius or an edge the shape doesn't have (named by number in the words). Every one
    // names the parameter at fault.
    let edge = |name: &'static str| Some(name);
    for (edges, radius, says, element, parameter) in [
        (&[1][..], 0.0, "radius", None, "radius"),
        (&[1], -2.0, "radius", None, "radius"),
        (&[1], f64::NAN, "radius", None, "radius"),
        (&[1], f64::INFINITY, "radius", None, "radius"),
        (&[], 2.0, "at least one edge", None, "edges"),
        (&[0], 2.0, "edge 0 doesn't exist", None, "edges"),
        (
            &[13],
            2.0,
            "edge 13 doesn't exist: the shape has edges 1 to 12",
            None,
            "edges",
        ),
        (
            &[1, 1],
            2.0,
            "Edge1 is listed twice",
            edge("Edge1"),
            "edges",
        ),
    ] {
        let err = fillet_through_worker(&mut w, edges, radius).unwrap_err();
        assert_eq!(
            err.code(),
            ErrorCode::InvalidInput,
            "{edges:?} {radius}: {err}"
        );
        assert!(err.message().contains(says), "{edges:?} {radius}: {err}");
        assert_eq!(
            err.elements(),
            element.as_slice(),
            "{edges:?} {radius}: {err}"
        );
        assert_eq!(
            err.parameter(),
            Some(parameter),
            "{edges:?} {radius}: {err}"
        );
    }
    assert!(!w.is_stopped());
}

#[test]
fn a_filleted_shape_crosses_the_boundary_intact() {
    // The B-rep a worker returns reads back as the same shape in-process.
    let mut w = worker();
    let brep = match w
        .call(
            Generation(1),
            Op::MakePrimitive(Primitive::Box {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            }),
            WAIT,
        )
        .unwrap()
    {
        Reply::Shape(shape) => shape,
        other => panic!("{other:?}"),
    };
    let rounded = match w
        .call(
            Generation(1),
            Op::Fillet {
                shape: brep,
                edges: vec![1],
                radius: 2.0,
            },
            WAIT,
        )
        .unwrap()
    {
        Reply::Built(built) => built.shape,
        other => panic!("{other:?}"),
    };
    let here =
        silico_kernel::summarize(&silico_kernel::from_brep(rounded.brep().as_bytes()).unwrap())
            .unwrap();
    let there = match w
        .call(
            Generation(1),
            Op::Summarize {
                shape: rounded.clone(),
            },
            WAIT,
        )
        .unwrap()
    {
        Reply::Summary(s) => s,
        other => panic!("{other:?}"),
    };
    // Bit for bit: the same deterministic kernel on both sides of the pipe.
    assert_eq!(here.volume.to_bits(), there.volume.to_bits());
    assert_eq!(here.area.to_bits(), there.area.to_bits());
    assert_eq!(
        (here.faces, here.edges, here.vertices),
        (there.faces, there.edges, there.vertices)
    );
    // The validity the reply carried is what a fresh check finds here.
    assert_eq!(rounded.valid(), Some(here.valid));
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}

/// The FreeCAD fillet tests that need only a box and a fillet, ported with their values.
#[test]
fn freecad_fillet_cube_to_sphere() {
    // PartDesign TestFillet.py testFilletCubeToSphere: every edge of a 10 cube at a radius
    // just under 5 leaves (nearly) a sphere of radius 5. FreeCAD asserts places=3.
    let cube = silico_kernel::make_box(10.0, 10.0, 10.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let s = silico_kernel::summarize(&silico_kernel::fillet(&cube, &all, 4.999999).unwrap().shape)
        .unwrap();
    let sphere = 4.0 / 3.0 * PI * 125.0;
    assert!(
        (s.volume - sphere).abs() < 5e-4,
        "volume {} vs {sphere}",
        s.volume
    );
    assert!(s.valid);
}

#[test]
fn freecad_fillet_every_edge_of_a_unit_cube() {
    // TopoShapeExpansion.cpp makeElementFillet: every edge of a unit cube at r = 0.05 has
    // area 5.739646. Analytic: six faces shrink to (1 − 2r)², twelve edges become quarter
    // cylinders, and eight corners make one sphere.
    let r: f64 = 0.05;
    let side = 1.0 - 2.0 * r;
    let area = 6.0 * side * side + 6.0 * PI * r * side + 4.0 * PI * r * r;
    let volume = side * side * side
        + 6.0 * r * side * side
        + 3.0 * PI * r * r * side
        + 4.0 / 3.0 * PI * r * r * r;
    let cube = silico_kernel::make_box(1.0, 1.0, 1.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let s =
        silico_kernel::summarize(&silico_kernel::fillet(&cube, &all, r).unwrap().shape).unwrap();
    assert!(
        (area - 5.739646).abs() < 1e-6,
        "the analytic area itself: {area}"
    );
    assert!(
        relative_error(s.area, area) <= 1e-9,
        "area {} vs {area}",
        s.area
    );
    assert!(
        relative_error(s.volume, volume) <= 1e-9,
        "volume {} vs {volume}",
        s.volume
    );
    assert!(s.valid);
}

#[test]
fn an_invalid_fillet_result_is_refused() {
    // OCCT reports these done, but the result isn't a valid solid (all edges of the
    // 10 × 20 × 30 box at r = 5.5 would claim a volume of 7792, more than the box).
    let mut w = worker();
    let all: Vec<u32> = (1..=12).collect();
    for radius in [5.01, 5.5, 9.0] {
        let err = fillet_through_worker(&mut w, &all, radius).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidResult, "r = {radius}: {err}");
        assert_eq!(err.message(), INVALID_RESULT, "r = {radius}: {err}");
        // Several edges: OCCT doesn't say which one is at fault, so the suggestion says
        // how to find it.
        assert!(err.elements().is_empty(), "r = {radius}: {err}");
        assert!(
            err.suggestion().unwrap().contains("one at a time"),
            "r = {radius}: {err}"
        );
    }
    // Just under the limit is fine, as in FreeCAD.
    assert!(fillet_through_worker(&mut w, &all, 4.99).is_ok());
}

#[test]
fn a_failed_fillet_names_only_the_edges_at_fault() {
    let mut w = worker();
    // One edge: that edge.
    let err = fillet_through_worker(&mut w, &[3], 11.0).unwrap_err();
    assert_eq!(err.elements(), ["Edge3"], "{err}");
    // Several: those OCCT couldn't build. Edge1 (along z) and Edge2 (along y) of the
    // 10 × 20 × 30 box each lie beside a face 10 wide, too narrow for r = 11; Edge11 (along
    // x, on the far side) lies beside faces 20 and 30 wide.
    let err = fillet_through_worker(&mut w, &[1, 2], 11.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.elements(), ["Edge1", "Edge2"], "{err}");
    let err = fillet_through_worker(&mut w, &[11, 1], 11.0).unwrap_err();
    assert_eq!(err.elements(), ["Edge1"], "{err}");
    assert!(fillet_through_worker(&mut w, &[11], 11.0).is_ok());
}

/// FreeCAD's names for the edges of a shape it makes with `make` (Python, `s` the shape)
/// that pass `test` (Python, of an edge `e`).
fn freecad_edges(make: &str, test: &str) -> Vec<String> {
    serde_json::from_value(oracle::freecad(&format!(
        "import json, Part, FreeCAD\nV = FreeCAD.Vector\ns = {make}\n\
         print('SILICO_ORACLE ' + json.dumps(['Edge%d' % (i + 1) for i, e in enumerate(s.Edges) if {test}]))\n"
    )))
    .unwrap()
}

#[test]
fn a_failed_fillet_names_every_edge_occt_couldnt_build() {
    // A 10 × 10 × 2 slab, every edge at r = 2.5: the horizontal edges lie beside the 2 mm
    // sides, too narrow for the radius; the vertical ones beside faces 10 wide. OCCT
    // reports the contours it couldn't build, and the core names exactly those edges.
    let horizontal = freecad_edges(
        "Part.makeBox(10, 10, 2)",
        "abs(e.tangentAt(e.FirstParameter).z) < 0.5",
    );
    assert_eq!(horizontal.len(), 8);
    let mut k = kernel();
    let slab = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: 2.0,
        })
        .unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let err = k.fillet(&slab, &all, 2.5).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.message(), "Fillet failed", "{err}");
    assert_eq!(err.elements(), horizontal, "{err}");
    assert_eq!(err.elements_total(), 8, "{err}");
    assert_eq!(
        err.suggestion(),
        Some(
            "if the fillet runs past a face beside these edges, try a smaller radius; if not, try another radius, or leave them out: OCCT can't fillet some edges at any size"
        )
    );
    // Each fails alone too, and the rest fillet together: the worker carries on.
    for edge in &horizontal {
        let n: u32 = edge.trim_start_matches("Edge").parse().unwrap();
        let alone = k.fillet(&slab, &[n], 2.5).unwrap_err();
        assert_eq!(alone.elements(), [edge.as_str()], "{alone}");
    }
    let vertical: Vec<u32> = (1..=12)
        .filter(|n| !horizontal.contains(&format!("Edge{n}")))
        .collect();
    assert!(k.fillet(&slab, &vertical, 2.5).is_ok());
}

#[test]
fn a_fillet_that_fails_only_where_its_edges_meet_names_no_edge() {
    // The same slab at r = 1: the top and bottom fillets would meet at mid-height on every
    // side, and OCCT can't build them together, though each edge fillets alone. OCCT finds
    // that by checking pairs of contours, and reports only the first of the pair it stopped
    // at (Edge2's, as OCCT 7.9.3 does), with no error status: not a way to tell which edges
    // are at fault, so none is named, and the suggestion is to find it one at a time.
    let mut k = kernel();
    let slab = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: 2.0,
        })
        .unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let err = k.fillet(&slab, &all, 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert!(err.elements().is_empty(), "{err}");
    assert!(err.suggestion().unwrap().contains("one at a time"), "{err}");
    for edge in all {
        assert!(k.fillet(&slab, &[edge], 1.0).is_ok(), "Edge{edge}");
    }
}

#[test]
fn a_fillet_that_fails_at_a_corner_names_the_edges_meeting_there() {
    // A pyramid on a 4 × 4 base, its apex 20 away, every edge at r = 2: OCCT can't blend the
    // apex, where the four slanted edges meet, and reports it, so those four are named.
    // (It reports Edge2's contour too, as the first of a pair it found colliding, with no
    // error status, which names nothing more.)
    let pyramid = "Part.makeWedge(0, 0, 0, 2, 2, 4, 20, 4, 2, 2)";
    let at_apex = freecad_edges(
        pyramid,
        "any((v.Point - V(2, 20, 2)).Length < 1e-9 for v in e.Vertexes)",
    );
    assert_eq!(at_apex.len(), 4);
    let text: String = serde_json::from_value(oracle::freecad(&format!(
        "import json, Part\nprint('SILICO_ORACLE ' + json.dumps({pyramid}.exportBrepToString()))\n"
    )))
    .unwrap();
    let shape = silico_kernel::from_text_brep(text.as_bytes()).unwrap();
    let data = ShapeData::unchecked(Blob::inline(silico_kernel::to_brep(&shape).unwrap()));
    let all: Vec<u32> = (1..=8).collect();
    let err = kernel().fillet(&data, &all, 2.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.elements(), at_apex, "{err}");
}

/// The fillet, through the worker.
fn fillet_brep(w: &mut Worker, edges: &[u32], radius: f64) -> ShapeData {
    let shape = match w
        .call(
            Generation(1),
            Op::MakePrimitive(Primitive::Box {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            }),
            WAIT,
        )
        .unwrap()
    {
        Reply::Shape(shape) => shape,
        other => panic!("{other:?}"),
    };
    match w
        .call(
            Generation(1),
            Op::Fillet {
                shape,
                edges: edges.to_vec(),
                radius,
            },
            WAIT,
        )
        .unwrap()
    {
        Reply::Built(built) => built.shape,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_fillet_is_byte_for_byte_deterministic() {
    let a = fillet_brep(&mut worker(), &[1, 2], 1.5);
    let mut w = worker();
    let b = fillet_brep(&mut w, &[1, 2], 1.5);
    let c = fillet_brep(&mut w, &[1, 2], 1.5);
    assert!(a == b && b == c);
}

#[test]
fn a_mesh_through_the_worker_matches_one_made_here() {
    let mut w = worker();
    let brep = fillet_brep(&mut w, &[1], 2.0);
    let there = match w
        .call(
            Generation(1),
            Op::Tessellate {
                shape: brep.clone(),
                deflection: 0.05,
                at_least_the_floor: false,
            },
            WAIT,
        )
        .unwrap()
    {
        Reply::Mesh(mesh) => mesh,
        other => panic!("{other:?}"),
    };
    let here = silico_kernel::tessellate(
        &silico_kernel::from_brep(brep.brep().as_bytes()).unwrap(),
        0.05,
    )
    .unwrap();
    assert_eq!(there, here);
    assert!(there.validate().is_ok());
    // Bad requests come back as invalid input, and the worker carries on.
    for (shape, deflection) in [
        (brep.clone(), 0.0),
        (brep.clone(), 1e-9),
        (
            ShapeData::unchecked(Blob::inline(b"not a shape".to_vec())),
            0.05,
        ),
    ] {
        let err = w
            .call(
                Generation(1),
                Op::Tessellate {
                    shape,
                    deflection,
                    at_least_the_floor: false,
                },
                WAIT,
            )
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{deflection}: {err}");
    }
    assert!(!w.is_stopped());
}

#[test]
fn a_fillet_that_cant_be_built_and_an_invalid_result_are_told_apart() {
    // Two failures an agent must handle differently: OCCT couldn't build the fillet, or
    // built one that isn't valid. The stable code says which, not just the message.
    let mut w = worker();
    let not_built = fillet_through_worker(&mut w, &[1, 2], 11.0).unwrap_err();
    let all: Vec<u32> = (1..=12).collect();
    let invalid = fillet_through_worker(&mut w, &all, 5.5).unwrap_err();
    assert_eq!(not_built.code(), ErrorCode::KernelFailed, "{not_built}");
    assert_eq!(not_built.message(), "Fillet failed", "{not_built}");
    assert_eq!(invalid.code(), ErrorCode::InvalidResult, "{invalid}");
    assert_eq!(invalid.message(), INVALID_RESULT, "{invalid}");
}

#[test]
fn a_problem_the_input_already_had_isnt_blamed_on_the_fillet() {
    // The open box is missing Face1 (x = 0), so it's invalid; Edge5 is on the far side.
    // FreeCAD's check (87293fac9b) looks only at the faces the fillet made or changed, so
    // it accepts this fillet, and so does Silico. The result is still the open box.
    let open = silico_kernel::test_ops::open_box().unwrap();
    let rounded = silico_kernel::fillet(&open, &[5], 2.0)
        .unwrap_or_else(|e| panic!("refused: {e} (element {:?})", e.element()))
        .shape;
    let s = silico_kernel::summarize(&rounded).unwrap();
    assert_eq!(s.faces, 6, "five faces and the fillet's");
    assert!(!s.valid);
}

#[test]
fn a_fillet_that_breaks_a_valid_input_is_still_refused_directly() {
    // The same check, called in-process: the refusal is its own kind, with a suggestion.
    let b = silico_kernel::make_box(10.0, 20.0, 30.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let err = silico_kernel::fillet(&b, &all, 5.5).err().unwrap();
    assert_eq!(
        err.kind(),
        silico_kernel::KernelErrorKind::InvalidResult,
        "{err}"
    );
    assert!(
        err.suggestion().unwrap().contains("smaller radius"),
        "{err}"
    );
}

#[test]
fn a_fillet_that_breaks_a_valid_filleted_box_is_refused() {
    // Valid where the second fillet changed it, but not as a whole: a "solid" larger than
    // the 6000 box it was cut from. The input was valid, so the result must be.
    let b = silico_kernel::make_box(10.0, 20.0, 30.0).unwrap();
    for (first_radius, second_edge) in [(3.0, 5), (1.0, 11)] {
        let once = silico_kernel::fillet(&b, &[1, 3, 5], first_radius)
            .unwrap()
            .shape;
        let s = silico_kernel::summarize(&once).unwrap();
        assert!(s.valid && s.volume < 6000.0, "the input is a valid fillet");
        match silico_kernel::fillet(&once, &[second_edge], 5.0) {
            Err(e) => assert_eq!(
                e.kind(),
                silico_kernel::KernelErrorKind::InvalidResult,
                "{e}"
            ),
            Ok(twice) => {
                let t = silico_kernel::summarize(&twice.shape).unwrap();
                panic!(
                    "r = {first_radius}, Edge{second_edge}: accepted, valid {} volume {}",
                    t.valid, t.volume
                );
            }
        }
    }
}

#[test]
fn a_valid_second_fillet_on_a_filleted_box_is_accepted() {
    // The stricter check mustn't refuse a good fillet on a modified solid. Two opposite
    // edges along z, r = 2: each removes (4 − π)·30 of volume and 120 − 30π of flat face,
    // adds 30π of quarter cylinder, and cuts (4 − π) from each of its two end faces.
    let b = silico_kernel::make_box(10.0, 20.0, 30.0).unwrap();
    let once = silico_kernel::fillet(&b, &[7], 2.0).unwrap().shape;
    let twice = silico_kernel::fillet(&once, &[3], 2.0)
        .unwrap_or_else(|e| panic!("refused: {e}"))
        .shape;
    let s = silico_kernel::summarize(&twice).unwrap();
    let volume = 6000.0 - 2.0 * (4.0 - PI) * 30.0;
    let area = 2200.0 - 2.0 * (120.0 - 30.0 * PI) - 4.0 * (4.0 - PI);
    assert!(
        relative_error(s.volume, volume) <= 1e-12,
        "volume {} vs {volume}",
        s.volume
    );
    assert!(
        relative_error(s.area, area) <= 1e-12,
        "area {} vs {area}",
        s.area
    );
    assert_eq!((s.faces, s.edges, s.vertices), (8, 18, 12));
    assert!(s.valid);
}

#[test]
fn each_shape_is_checked_as_a_whole_at_most_once() {
    // A shape keeps its validity once known, from a check or from the worker that made it.
    let b = silico_kernel::make_box(10.0, 20.0, 30.0).unwrap();
    assert!(b.is_valid().unwrap());
    let open = silico_kernel::test_ops::open_box().unwrap();
    assert!(!open.is_valid().unwrap());
    let said_invalid = silico_kernel::make_box(10.0, 20.0, 30.0)
        .unwrap()
        .with_validity(false);
    assert!(!said_invalid.is_valid().unwrap());
    assert!(!silico_kernel::summarize(&said_invalid).unwrap().valid);
    // A fillet doesn't check its input again: told the filleted box is invalid, it applies
    // only FreeCAD's changed-faces check, and the broken second fillet (refused when the
    // input is known valid, a_fillet_that_breaks_a_valid_filleted_box_is_refused) gets
    // through. Its result's own check is kept with it.
    let once = silico_kernel::fillet(&b, &[1, 3, 5], 3.0)
        .unwrap()
        .shape
        .with_validity(false);
    let twice = silico_kernel::fillet(&once, &[5], 5.0).unwrap().shape;
    assert!(!twice.is_valid().unwrap());
    assert!(!silico_kernel::summarize(&twice).unwrap().valid);
}

#[test]
fn every_edge_refused_reaches_the_core_as_an_element() {
    // M2.0: a sphere's three edges (FreeCAD's makeSphere: a seam and two degenerate poles)
    // are all ones OCCT can't fillet; one refusal names every one, as data the worker
    // sends, with the names and the words written by the core.
    let mut w = kernel();
    let sphere = w.make(Primitive::Sphere { radius: 5.0 }).unwrap();
    let err = w.fillet(&sphere, &[1, 2, 3], 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(err.elements(), ["Edge1", "Edge2", "Edge3"], "{err}");
    assert_eq!(
        err.suggestion(),
        Some("leave Edge1, Edge2, and Edge3 out of the fillet"),
        "{err}"
    );
    assert!(
        err.message()
            .ends_with("; OCCT won't fillet Edge2, Edge3 either"),
        "{err}"
    );
}

#[test]
fn a_refusal_that_doesnt_fit_the_request_stops_the_worker() {
    // A broken worker refuses a fillet of Edge1 as one of Edge7: the core checks the
    // refusal against the request before describing it, so it's a protocol error, the
    // worker is stopped, and nothing is said about Edge7.
    let mut w = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker"))).unwrap();
    w.call(
        Generation(0),
        Op::Debug(DebugOp::Misbehave(Misbehavior::ForgedRefusal)),
        WAIT,
    )
    .unwrap();
    let mut k = KernelWorker::from_worker(w);
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let err = k.fillet(&shape, &[1], 2.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert_eq!(
        err.message(),
        "the kernel worker's refusal of Fillet doesn't fit the request"
    );
    assert_eq!(
        err.cause(),
        Some("it names edge 7, which the request doesn't list"),
        "{err}"
    );
    assert!(err.elements().is_empty(), "{err}");
    assert!(
        err.suggestion()
            .is_some_and(|s| s.starts_with("this is a bug in Silico's kernel worker")),
        "{err}"
    );
    let after = k.summarize(&shape).unwrap_err();
    assert_eq!(after.code(), ErrorCode::WorkerCrashed, "{after}");
}

#[test]
fn a_fillet_that_fails_where_a_boss_meets_its_plate_names_no_edge() {
    // A 10 × 10 × 2 plate with a boss of radius 3 and height 1 on its top, every edge but
    // the boss's seam (which is refused before building) at r = 0.5: OCCT can't build it
    // and names no contour it couldn't build, so no edge is named, and the suggestion is to
    // find it one at a time. Each edge fillets alone, so trying them alone finds none
    // either (docs/ports/fillet.md).
    let out: serde_json::Value = oracle::freecad(
        "import json, Part, FreeCAD\n\
         s = Part.makeBox(10, 10, 2).fuse(Part.makeCylinder(3, 1, FreeCAD.Vector(5, 5, 2)))\n\
         seams = [i + 1 for i, e in enumerate(s.Edges) if any(e.isSeam(f) for f in s.Faces if any(e.isSame(x) for x in f.Edges))]\n\
         print('SILICO_ORACLE ' + json.dumps({'brep': s.exportBrepToString(), 'edges': len(s.Edges), 'seams': seams}))\n",
    );
    let seams: Vec<u32> = serde_json::from_value(out["seams"].clone()).unwrap();
    let count: u32 = serde_json::from_value(out["edges"].clone()).unwrap();
    assert_eq!(seams.len(), 1, "{seams:?}");
    let shape = silico_kernel::from_text_brep(out["brep"].as_str().unwrap().as_bytes()).unwrap();
    let data = ShapeData::unchecked(Blob::inline(silico_kernel::to_brep(&shape).unwrap()));
    let edges: Vec<u32> = (1..=count).filter(|e| !seams.contains(e)).collect();
    let mut k = kernel();
    let err = k.fillet(&data, &edges, 0.5).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert!(err.elements().is_empty(), "{err}");
    assert!(err.suggestion().unwrap().contains("one at a time"), "{err}");
    for edge in edges {
        assert!(k.fillet(&data, &[edge], 0.5).is_ok(), "Edge{edge}");
    }
}
