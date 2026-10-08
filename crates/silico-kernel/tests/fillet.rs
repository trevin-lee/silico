//! M0.8: the fillet, checked against analytic values in-process, and against FreeCAD
//! through the worker (the M0 exit test: a request crosses the process boundary, the cxx
//! bridge, and the deterministic OCCT build, and agrees with FreeCAD).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/imports.rs"]
mod imports;
#[path = "common/oracle.rs"]
mod oracle;

use std::f64::consts::PI;
use std::path::Path;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::worker::Worker;
use silico_protocol::{Blob, Op, Reply, ShapeSummary};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TIMEOUT: Duration = Duration::from_secs(30);

fn relative_error(actual: f64, expected: f64) -> f64 {
    ((actual - expected) / expected).abs()
}

#[test]
fn box_fillet_analytic() {
    // Edge 1 of a 10 × 20 × 30 box runs along z, length 30 (FreeCAD agrees; see below).
    let rounded = silico_kernel::fillet(
        &silico_kernel::make_box(10.0, 20.0, 30.0).unwrap(),
        &[1],
        2.0,
    )
    .unwrap();
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
        1,
        Op::MakeBox {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        },
        TIMEOUT,
    )? {
        Reply::Shape { brep } => brep,
        other => panic!("MakeBox replied {other:?}"),
    };
    let rounded = match w.call(
        1,
        Op::Fillet {
            shape,
            edges: edges.to_vec(),
            radius,
        },
        TIMEOUT,
    )? {
        Reply::Shape { brep } => brep,
        other => panic!("Fillet replied {other:?}"),
    };
    match w.call(1, Op::Summarize { shape: rounded }, TIMEOUT)? {
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
    // Below OCCT's 1e-7 tolerance, and absurdly small or large: the two agree.
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
    for (&(edges, radius), theirs) in CASES.iter().zip(&freecad) {
        let ours = fillet_through_worker(&mut w, edges, radius);
        let case = format!("edges {edges:?}, radius {radius}");
        match (&ours, theirs.get("error")) {
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
                assert_eq!(e.code(), ErrorCode::KernelFailed, "{case}: {e}");
                assert!(
                    e.cause().unwrap().contains("isn't a valid solid"),
                    "{case}: {e}"
                );
                refused_invalid += 1;
            }
            (ours, theirs) => panic!("{case}: Silico gave {ours:?}, FreeCAD gave {theirs:?}"),
        }
    }
    // The comparison isn't vacuous: most cases succeed on both sides, and FreeCAD's
    // invalid results really are refused.
    assert!(compared >= 8, "only {compared} cases compared");
    assert_eq!(refused_invalid, 2);
}

#[test]
fn fillet_too_large_is_structured_error() {
    // r = 11 on an edge whose adjacent 10-wide face can't hold it. FreeCAD refuses it too
    // (box_fillet_through_worker_matches_freecad).
    let mut w = worker();
    let err = fillet_through_worker(&mut w, &[1], 11.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.message(), "Fillet failed");
    assert_eq!(err.element(), Some("Edge1"));
    assert!(
        err.cause().unwrap().contains("StdFail_NotDone")
            || err.cause().unwrap().contains("couldn't be built"),
        "{err}"
    );
    // The worker carries on.
    assert!(fillet_through_worker(&mut w, &[1], 2.0).is_ok());
}

#[test]
fn bad_fillet_requests_are_invalid_input() {
    let mut w = worker();
    // The element is named only when an edge is at fault, not for a bad radius.
    let edge = |name: &'static str| Some(name);
    for (edges, radius, says, element) in [
        (&[1][..], 0.0, "radius", None),
        (&[1], -2.0, "radius", None),
        (&[1], f64::NAN, "radius", None),
        (&[1], f64::INFINITY, "radius", None),
        (&[], 2.0, "at least one edge", None),
        (&[0], 2.0, "Edge0 doesn't exist", edge("Edge0")),
        (
            &[13],
            2.0,
            "Edge13 doesn't exist: the shape has edges 1 to 12",
            edge("Edge13"),
        ),
        (&[1, 1], 2.0, "Edge1 is listed twice", edge("Edge1")),
    ] {
        let err = fillet_through_worker(&mut w, edges, radius).unwrap_err();
        assert_eq!(
            err.code(),
            ErrorCode::InvalidInput,
            "{edges:?} {radius}: {err}"
        );
        assert!(err.message().contains(says), "{edges:?} {radius}: {err}");
        assert_eq!(err.element(), element, "{edges:?} {radius}: {err}");
    }
    assert!(!w.is_stopped());
}

#[test]
fn a_filleted_shape_crosses_the_boundary_intact() {
    // The B-rep a worker returns reads back as the same shape in-process.
    let mut w = worker();
    let brep = match w
        .call(
            1,
            Op::MakeBox {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Shape { brep } => brep,
        other => panic!("{other:?}"),
    };
    let rounded = match w
        .call(
            1,
            Op::Fillet {
                shape: brep,
                edges: vec![1],
                radius: 2.0,
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Shape { brep } => brep,
        other => panic!("{other:?}"),
    };
    let here =
        silico_kernel::summarize(&silico_kernel::from_brep(rounded.as_bytes()).unwrap()).unwrap();
    let there = match w
        .call(
            1,
            Op::Summarize {
                shape: Blob::inline(rounded.as_bytes().to_vec()),
            },
            TIMEOUT,
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
    let s =
        silico_kernel::summarize(&silico_kernel::fillet(&cube, &all, 4.999999).unwrap()).unwrap();
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
    let s = silico_kernel::summarize(&silico_kernel::fillet(&cube, &all, r).unwrap()).unwrap();
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
        assert_eq!(err.code(), ErrorCode::KernelFailed, "r = {radius}: {err}");
        assert!(
            err.cause().unwrap().contains("isn't a valid solid"),
            "r = {radius}: {err}"
        );
        // Several edges: OCCT doesn't say which one is at fault.
        assert_eq!(err.element(), None, "r = {radius}: {err}");
    }
    // Just under the limit is fine, as in FreeCAD.
    assert!(fillet_through_worker(&mut w, &all, 4.99).is_ok());
}

#[test]
fn a_failed_fillet_names_only_the_edge_at_fault() {
    let mut w = worker();
    // One edge: that edge.
    let err = fillet_through_worker(&mut w, &[3], 11.0).unwrap_err();
    assert_eq!(err.element(), Some("Edge3"), "{err}");
    // Several: OCCT doesn't say which, so none.
    let err = fillet_through_worker(&mut w, &[1, 2], 11.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.element(), None, "{err}");
}

/// The fillet's bytes, through the worker.
fn fillet_brep(w: &mut Worker, edges: &[u32], radius: f64) -> Blob {
    let shape = match w
        .call(
            1,
            Op::MakeBox {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Shape { brep } => brep,
        other => panic!("{other:?}"),
    };
    match w
        .call(
            1,
            Op::Fillet {
                shape,
                edges: edges.to_vec(),
                radius,
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Shape { brep } => brep,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_fillet_is_byte_for_byte_deterministic() {
    let a = fillet_brep(&mut worker(), &[1, 2], 1.5);
    let mut w = worker();
    let b = fillet_brep(&mut w, &[1, 2], 1.5);
    let c = fillet_brep(&mut w, &[1, 2], 1.5);
    assert!(a.as_bytes() == b.as_bytes() && b.as_bytes() == c.as_bytes());
}
