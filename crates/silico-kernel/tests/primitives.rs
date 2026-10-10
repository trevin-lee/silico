//! M1.3: solid primitives through the core's kernel API and a real worker. Volumes, areas,
//! and centers are analytic; invalid values are refused before any request is sent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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

use kernel::kernel;
use numbers::relative_error;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::kernel::{KernelWorker, Primitive, ShapeData};
use silico_core::worker::Worker;
use silico_protocol::{Blob, DebugOp, Generation, Misbehavior, Op};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// Curved faces are integrated numerically; OCCT gets these to well within this.
const CURVED: f64 = 1e-9;

struct Expected {
    primitive: Primitive,
    volume: f64,
    area: f64,
    center: [f64; 3],
}

fn analytic() -> Vec<Expected> {
    let frustum_volume = PI * 10.0 / 3.0 * (25.0 + 10.0 + 4.0);
    vec![
        Expected {
            primitive: Primitive::Cylinder {
                radius: 5.0,
                height: 10.0,
            },
            // πr²h; 2πr² + 2πrh.
            volume: 250.0 * PI,
            area: 150.0 * PI,
            center: [0.0, 0.0, 5.0],
        },
        Expected {
            primitive: Primitive::Sphere { radius: 5.0 },
            // 4/3 πr³; 4πr².
            volume: 500.0 * PI / 3.0,
            area: 100.0 * PI,
            center: [0.0, 0.0, 0.0],
        },
        Expected {
            primitive: Primitive::Cone {
                radius1: 5.0,
                radius2: 0.0,
                height: 10.0,
            },
            // πr²h/3; πr² + πr·√(r² + h²); the centroid a quarter of the way up.
            volume: 250.0 * PI / 3.0,
            area: 25.0 * PI + 5.0 * PI * 125f64.sqrt(),
            center: [0.0, 0.0, 2.5],
        },
        Expected {
            primitive: Primitive::Cone {
                radius1: 5.0,
                radius2: 2.0,
                height: 10.0,
            },
            // A frustum: πh/3 (r₁² + r₁r₂ + r₂²); π(r₁² + r₂²) + π(r₁ + r₂)·√((r₁ − r₂)² + h²);
            // centroid h (r₁² + 2r₁r₂ + 3r₂²) / 4(r₁² + r₁r₂ + r₂²) up.
            volume: frustum_volume,
            area: 29.0 * PI + 7.0 * PI * 109f64.sqrt(),
            center: [0.0, 0.0, 10.0 * (25.0 + 20.0 + 12.0) / (4.0 * 39.0)],
        },
        Expected {
            primitive: Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 2.0,
            },
            // 2π²Rr²; 4π²Rr.
            volume: 80.0 * PI * PI,
            area: 80.0 * PI * PI,
            center: [0.0, 0.0, 0.0],
        },
    ]
}

#[test]
fn primitives_have_their_analytic_volume_area_and_center() {
    let mut k = kernel();
    for e in analytic() {
        let shape = k.make(e.primitive).unwrap();
        let s = k.summarize(&shape).unwrap();
        let what = format!("{:?}", e.primitive);
        assert!(s.valid, "{what}");
        assert_eq!(shape.valid(), Some(true), "{what}");
        assert!(
            relative_error(s.volume, e.volume) <= CURVED,
            "{what}: volume {} vs {}",
            s.volume,
            e.volume
        );
        assert!(
            relative_error(s.area, e.area) <= CURVED,
            "{what}: area {} vs {}",
            s.area,
            e.area
        );
        // Absolute, since some centers are at zero; scaled to the part's size (10).
        for (got, want) in s.center.into_iter().zip(e.center) {
            assert!(
                (got - want).abs() <= 10.0 * CURVED,
                "{what}: center {:?} vs {:?}",
                s.center,
                e.center
            );
        }
    }
}

/// Every parameter of every primitive, with a valid value for each.
fn valid_primitives() -> Vec<Primitive> {
    vec![
        Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        },
        Primitive::Cylinder {
            radius: 1.0,
            height: 2.0,
        },
        Primitive::Sphere { radius: 1.0 },
        Primitive::Cone {
            radius1: 1.0,
            radius2: 2.0,
            height: 3.0,
        },
        Primitive::Torus {
            major_radius: 3.0,
            minor_radius: 1.0,
        },
    ]
}

/// `primitive` with the parameter named `parameter` set to `value`.
fn with(primitive: Primitive, parameter: &str, value: f64) -> Primitive {
    let mut p = primitive;
    let slot = match &mut p {
        Primitive::Box { dx, dy, dz } => match parameter {
            "dx" => dx,
            "dy" => dy,
            "dz" => dz,
            _ => panic!("a box has no {parameter}"),
        },
        Primitive::Cylinder { radius, height } => match parameter {
            "radius" => radius,
            "height" => height,
            _ => panic!("a cylinder has no {parameter}"),
        },
        Primitive::Sphere { radius } => match parameter {
            "radius" => radius,
            _ => panic!("a sphere has no {parameter}"),
        },
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => match parameter {
            "radius1" => radius1,
            "radius2" => radius2,
            "height" => height,
            _ => panic!("a cone has no {parameter}"),
        },
        Primitive::Torus {
            major_radius,
            minor_radius,
        } => match parameter {
            "major_radius" => major_radius,
            "minor_radius" => minor_radius,
            _ => panic!("a torus has no {parameter}"),
        },
    };
    *slot = value;
    p
}

fn parameters(primitive: Primitive) -> &'static [&'static str] {
    match primitive {
        Primitive::Box { .. } => &["dx", "dy", "dz"],
        Primitive::Cylinder { .. } => &["radius", "height"],
        Primitive::Sphere { .. } => &["radius"],
        Primitive::Cone { .. } => &["radius1", "radius2", "height"],
        Primitive::Torus { .. } => &["major_radius", "minor_radius"],
    }
}

#[test]
fn invalid_params_never_reach_the_worker() {
    let mut k = kernel();
    for primitive in valid_primitives() {
        for &parameter in parameters(primitive) {
            // A cone's radii may be zero (one at a time); its other lengths, and every
            // other, may not. Nothing may be a length at or below OCCT's tolerance.
            let zero_ok = matches!(parameter, "radius1" | "radius2");
            // Lengths above a kilometre too: OCCT measures some of them wrongly.
            let mut bad = vec![
                -1.0,
                -1e-300,
                1e-7,
                5e-324,
                1e6 + 0.001,
                1e12,
                f64::MAX,
                f64::NAN,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ];
            if !zero_ok {
                bad.extend([0.0, -0.0]);
            }
            for value in bad {
                let p = with(primitive, parameter, value);
                let err = k.make(p).unwrap_err();
                assert_eq!(err.code(), ErrorCode::InvalidInput, "{p:?}: {err}");
                assert!(
                    err.message()
                        .starts_with(&format!("{} {parameter} must be", primitive.name())),
                    "{p:?}: {err}"
                );
                assert!(
                    err.message().contains("at most 1e6 mm") && err.message().len() < 200,
                    "{p:?}: {err}"
                );
                assert!(err.suggestion().is_some(), "{p:?}: {err}");
            }
        }
    }
    // Combinations: a cone with no radius at either end, a torus whose tube crosses its
    // axis.
    let combinations = [
        (
            Primitive::Cone {
                radius1: 0.0,
                radius2: 0.0,
                height: 10.0,
            },
            "cone radius1 must be",
        ),
        (
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 12.0,
            },
            "torus minor_radius must be",
        ),
        (
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 10.000_000_2,
            },
            "torus minor_radius must be",
        ),
    ];
    for (p, says) in combinations {
        let err = k.make(p).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{p:?}: {err}");
        assert!(err.message().starts_with(says), "{p:?}: {err}");
    }
    assert_eq!(k.requests_sent(), 0, "an invalid value reached the worker");
    // And the kernel works: the checks aren't refusing everything.
    for primitive in valid_primitives() {
        assert!(k.make(primitive).is_ok(), "{primitive:?}");
    }
    assert_eq!(k.requests_sent(), valid_primitives().len() as u64);
}

#[test]
fn the_worker_refuses_invalid_params_itself() {
    // A client that skips the core's check still can't get them past the worker.
    let mut w = Worker::spawn(Path::new(WORKER)).unwrap();
    for primitive in valid_primitives() {
        for &parameter in parameters(primitive) {
            let p = with(primitive, parameter, f64::NAN);
            let err = w
                .call(Generation(0), Op::MakePrimitive(p), Duration::from_secs(30))
                .unwrap_err();
            assert_eq!(err.code(), ErrorCode::InvalidInput, "{p:?}: {err}");
            assert!(err.message().contains(parameter), "{p:?}: {err}");
        }
    }
    assert!(!w.is_stopped());
}

#[test]
fn a_cone_may_come_to_a_point_at_either_end() {
    let mut k = kernel();
    for (radius1, radius2) in [(5.0, 0.0), (0.0, 5.0)] {
        let shape = k
            .make(Primitive::Cone {
                radius1,
                radius2,
                height: 10.0,
            })
            .unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid);
        assert!(
            relative_error(s.volume, 250.0 * PI / 3.0) <= CURVED,
            "{}",
            s.volume
        );
    }
}

/// A length spread evenly over the decades from 1e-3 to 1e4.
fn length() -> impl Strategy<Value = f64> {
    (1.0f64..10.0, -3i32..=3).prop_map(|(mantissa, exponent)| {
        let ten = if exponent < 0 { 0.1 } else { 10.0 };
        (0..exponent.unsigned_abs()).fold(mantissa, |x, _| x * ten)
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Any valid size from 1e-3 to 1e4, every decade sampled, gives the formula's volume:
    /// pointed cones and tori up to touching their axis included.
    #[test]
    fn primitive_volume_property(
        a in length(),
        b in length(),
        c in length(),
        point_at_top in any::<bool>(),
        // The tube's radius as a fraction of the ring's, up to touching the axis.
        tube in 1e-3f64..=1.0,
    ) {
        let mut k = kernel();
        let (r1, r2) = if point_at_top { (a, 0.0) } else { (0.0, a) };
        let cases = [
            (Primitive::Box { dx: a, dy: b, dz: c }, a * b * c),
            (Primitive::Cylinder { radius: a, height: b }, PI * a * a * b),
            (Primitive::Sphere { radius: a }, 4.0 / 3.0 * PI * a * a * a),
            (
                Primitive::Cone { radius1: a, radius2: b, height: c },
                PI * c / 3.0 * (a * a + a * b + b * b),
            ),
            (
                Primitive::Cone { radius1: r1, radius2: r2, height: c },
                PI * c / 3.0 * a * a,
            ),
            (
                Primitive::Torus { major_radius: b, minor_radius: b * tube },
                2.0 * PI * PI * b * (b * tube) * (b * tube),
            ),
        ];
        for (primitive, volume) in cases {
            let shape = k.make(primitive).unwrap();
            let s = k.summarize(&shape).unwrap();
            prop_assert!(s.valid, "{:?}", primitive);
            prop_assert!(
                relative_error(s.volume, volume) <= 1e-6,
                "{:?}: volume {} vs {}", primitive, s.volume, volume
            );
        }
    }
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}

#[test]
fn a_torus_may_touch_its_axis() {
    // A horn torus: the tube's radius equal to the ring's, or within OCCT's tolerance of
    // it. Volume 2π²Rr² with R = r = 10.
    let mut k = kernel();
    for minor_radius in [10.0, 10.000_000_05] {
        let shape = k
            .make(Primitive::Torus {
                major_radius: 10.0,
                minor_radius,
            })
            .unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{minor_radius}");
        let volume = 2.0 * PI * PI * 10.0 * minor_radius * minor_radius;
        assert!(
            relative_error(s.volume, volume) <= 1e-6,
            "{minor_radius}: {} vs {volume}",
            s.volume
        );
    }
}

#[test]
fn the_extremes_allowed_still_measure_right() {
    // The largest and smallest lengths together: OCCT measures some solids wrongly when
    // their lengths span more orders of magnitude than this (a torus of radii 1e12 and
    // 1e-3 has a negative volume). Analytic volumes.
    let mut k = kernel();
    let (big, small) = (1e6, 1.01e-7);
    let cases = [
        (
            Primitive::Torus {
                major_radius: big,
                minor_radius: small,
            },
            2.0 * PI * PI * big * small * small,
        ),
        (
            Primitive::Cone {
                radius1: big,
                radius2: 0.0,
                height: small,
            },
            PI * big * big * small / 3.0,
        ),
        (
            Primitive::Cylinder {
                radius: small,
                height: big,
            },
            PI * small * small * big,
        ),
    ];
    for (primitive, volume) in cases {
        let shape = k.make(primitive).unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{primitive:?}");
        assert!(
            relative_error(s.volume, volume) <= 1e-3,
            "{primitive:?}: {} vs {volume}",
            s.volume
        );
    }
}

#[test]
fn a_negative_zero_radius_builds_the_same_cone() {
    // -0.0 is a zero radius; the shape's bytes don't depend on the sign.
    let mut k = kernel();
    let cone = |radius1| Primitive::Cone {
        radius1,
        radius2: 5.0,
        height: 10.0,
    };
    let a = k.make(cone(0.0)).unwrap();
    let b = k.make(cone(-0.0)).unwrap();
    assert!(a == b, "the B-reps differ");
}

/// The primitives compared with FreeCAD, as (FreeCAD feature type, its properties, Silico's
/// primitive).
fn oracle_cases() -> Vec<(&'static str, String, Primitive)> {
    let cone = |r1: f64, r2: f64| {
        (
            "Part::Cone",
            format!("Radius1={r1:?}, Radius2={r2:?}, Height=10.0"),
            Primitive::Cone {
                radius1: r1,
                radius2: r2,
                height: 10.0,
            },
        )
    };
    let torus = |r: f64| {
        (
            "Part::Torus",
            format!("Radius1=10.0, Radius2={r:?}"),
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: r,
            },
        )
    };
    vec![
        (
            "Part::Box",
            "Length=10.0, Width=20.0, Height=30.0".to_owned(),
            Primitive::Box {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            },
        ),
        (
            "Part::Cylinder",
            "Radius=5.0, Height=10.0".to_owned(),
            Primitive::Cylinder {
                radius: 5.0,
                height: 10.0,
            },
        ),
        (
            "Part::Sphere",
            "Radius=5.0".to_owned(),
            Primitive::Sphere { radius: 5.0 },
        ),
        cone(5.0, 0.0),
        cone(0.0, 5.0),
        cone(5.0, 2.0),
        // Equal radii, and radii within OCCT's tolerance: a cylinder, built directly.
        cone(5.0, 5.0),
        cone(5.0, 5.000_000_05),
        // Just past the switch: a cone again.
        cone(5.0, 5.000_000_2),
        torus(2.0),
        // A horn torus: the tube touches the axis.
        torus(10.0),
    ]
}

/// The bounding box of `points`, as [min x, min y, min z, max x, max y, max z].
fn bounds(points: impl Iterator<Item = [f64; 3]>) -> [f64; 6] {
    points.fold(
        [f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN],
        |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].min(p[2]),
                b[3].max(p[0]),
                b[4].max(p[1]),
                b[5].max(p[2]),
            ]
        },
    )
}

#[test]
fn primitives_match_freecad_face_for_face_and_edge_for_edge() {
    // FreeCAD's Part features, as a person makes them: volume, area, center, topology,
    // validity, and each face's and edge's extent, in order. Silico builds each primitive
    // as FreeCAD does, so Face1, Edge1, ... are the same faces and edges.
    let cases = oracle_cases();
    let objects: Vec<String> = cases
        .iter()
        .map(|(kind, props, _)| {
            format!(
                "o = doc.addObject('{kind}'); [setattr(o, k, v) for k, v in dict({props}).items()]; objs.append(o)"
            )
        })
        .collect();
    let script = format!(
        r#"
import json, FreeCAD
doc = FreeCAD.newDocument()
objs = []
{}
doc.recompute()
def box(b): return [b.XMin, b.YMin, b.ZMin, b.XMax, b.YMax, b.ZMax]
out = []
for o in objs:
    s = o.Shape
    out.append(dict(
        volume=s.Volume, area=s.Area, center=list(s.Solids[0].CenterOfMass),
        faces=[box(f.optimalBoundingBox(False, False)) for f in s.Faces],
        edges=[None if e.Degenerated else box(e.optimalBoundingBox(False, False)) for e in s.Edges],
        vertices=len(s.Vertexes), valid=s.isValid()))
print("SILICO_ORACLE " + json.dumps(out))
"#,
        objects.join("\n")
    );
    let theirs = oracle::freecad(&script);
    let mut k = kernel();
    // Fine enough that a mesh's extent is within this of the true one.
    let deflection = 0.01;
    let near = 0.03;
    for ((kind, props, primitive), theirs) in cases.iter().zip(theirs.as_array().unwrap()) {
        let what = format!("{kind}({props})");
        let shape = k.make(*primitive).unwrap();
        let s = k.summarize(&shape).unwrap();
        let mesh = k.tessellate(&shape, deflection).unwrap();
        let num = |v: &serde_json::Value| v.as_f64().unwrap();
        assert!(
            relative_error(s.volume, num(&theirs["volume"])) <= 1e-9,
            "{what}: volume {} vs {}",
            s.volume,
            theirs["volume"]
        );
        assert!(
            relative_error(s.area, num(&theirs["area"])) <= 1e-9,
            "{what}: area {} vs {}",
            s.area,
            theirs["area"]
        );
        // Silico integrates adaptively (to a relative 1e-9), FreeCAD with a fixed order of
        // points: they agree to 1e-9 except at a horn torus's singular point, where the
        // adaptive center is 7.5e-6 off the axis (of a torus 40 across).
        let horn = matches!(primitive, Primitive::Torus { major_radius, minor_radius } if major_radius == minor_radius);
        let center_tolerance = if horn { 1e-5 } else { 1e-9 };
        for (ours, theirs) in s.center.iter().zip(theirs["center"].as_array().unwrap()) {
            assert!(
                (ours - num(theirs)).abs() <= center_tolerance,
                "{what}: center {:?}",
                s.center
            );
        }
        assert_eq!(
            s.valid,
            theirs["valid"].as_bool().unwrap(),
            "{what}: validity"
        );
        assert_eq!(
            s.vertices as u64,
            theirs["vertices"].as_u64().unwrap(),
            "{what}"
        );

        let their_faces = theirs["faces"].as_array().unwrap();
        assert_eq!(
            mesh.face_triangles.len(),
            their_faces.len(),
            "{what}: faces"
        );
        let mut first = 0usize;
        for (i, (&count, theirs)) in mesh.face_triangles.iter().zip(their_faces).enumerate() {
            let triangles = &mesh.triangles[first..first + count as usize];
            first += count as usize;
            let ours = bounds(
                triangles
                    .iter()
                    .flatten()
                    .map(|&v| mesh.positions[v as usize]),
            );
            for (a, b) in ours.iter().zip(theirs.as_array().unwrap()) {
                assert!(
                    (a - num(b)).abs() <= near,
                    "{what}: Face{} {ours:?} vs {theirs}",
                    i + 1
                );
            }
        }

        let their_edges = theirs["edges"].as_array().unwrap();
        assert_eq!(mesh.edges().count(), their_edges.len(), "{what}: edges");
        for (i, (points, theirs)) in mesh.edges().zip(their_edges).enumerate() {
            if theirs.is_null() {
                assert!(points.is_empty(), "{what}: Edge{} isn't degenerate", i + 1);
                continue;
            }
            let ours = bounds(points.iter().copied());
            for (a, b) in ours.iter().zip(theirs.as_array().unwrap()) {
                assert!(
                    (a - num(b)).abs() <= near,
                    "{what}: Edge{} {ours:?} vs {theirs}",
                    i + 1
                );
            }
        }
    }
}

const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

/// A shape that isn't one, for operations whose answer doesn't matter.
fn shape() -> silico_core::kernel::ShapeData {
    silico_core::kernel::ShapeData::unchecked(silico_protocol::Blob::inline(vec![]))
}

/// A kernel on the test worker, told to misbehave in this way.
fn misbehaving(how: Misbehavior) -> KernelWorker {
    let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    w.call(
        Generation(0),
        Op::Debug(DebugOp::Misbehave(how)),
        Duration::from_secs(30),
    )
    .unwrap();
    KernelWorker::from_worker(w)
}

#[test]
fn a_mesh_that_doesnt_hold_together_is_refused() {
    // A client indexes into a mesh; the kernel API checks it first, so a broken one is an
    // error, never a panic in the app.
    let mut k = misbehaving(Misbehavior::BadMesh);
    let shape = k
        .make(Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        })
        .unwrap();
    let err = k.tessellate(&shape, 0.1).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(err.cause().unwrap().contains("past the 3"), "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    // A worker that sends one can't be trusted: it's stopped.
    let after = k.summarize(&shape).unwrap_err();
    assert_eq!(after.code(), ErrorCode::WorkerCrashed, "{after}");
}

#[test]
fn a_reply_of_the_wrong_kind_is_a_protocol_error() {
    // Each operation, on a worker that answers everything with Done: a protocol error that
    // names the operation and the kind of reply (never its contents), and the worker is
    // stopped, since its replies can't be trusted.
    type Call = fn(&mut KernelWorker) -> silico_core::Error;
    let calls: [(&str, Call); 4] = [
        ("MakeBox", |k| {
            k.make(Primitive::Box {
                dx: 1.0,
                dy: 2.0,
                dz: 3.0,
            })
            .unwrap_err()
        }),
        ("Summarize", |k| k.summarize(&shape()).unwrap_err()),
        ("Tessellate", |k| k.tessellate(&shape(), 0.1).unwrap_err()),
        ("Fillet", |k| k.fillet(&shape(), &[1], 1.0).unwrap_err()),
    ];
    for (name, call) in calls {
        let mut k = misbehaving(Misbehavior::WrongReply);
        let err = call(&mut k);
        assert_eq!(err.code(), ErrorCode::ProtocolError, "{name}: {err}");
        assert!(err.message().contains(name), "{name}: {err}");
        assert_eq!(err.cause(), Some("a Done reply"), "{name}: {err}");
        let after = k.summarize(&shape()).unwrap_err();
        assert_eq!(after.code(), ErrorCode::WorkerCrashed, "{name}: {after}");
    }
}

#[test]
fn the_kernel_finds_its_worker() {
    // Next to the running executable, or one directory up: a test binary's worker is in
    // the directory above it. (Named in the environment, it wouldn't be looking.)
    assert!(
        std::env::var_os("SILICO_KERNEL_WORKER").is_none(),
        "unset SILICO_KERNEL_WORKER to run this test"
    );
    let mut k = KernelWorker::start().unwrap();
    let shape = k
        .make(Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        })
        .unwrap();
    assert!(relative_error(k.summarize(&shape).unwrap().volume, 6.0) <= 1e-12);
}

#[test]
fn a_cone_just_past_the_cylinder_switch_is_a_cone() {
    // Radii more than 1e-7 apart make a cone (a frustum here), not a cylinder: the
    // frustum's volume, πh/3 (r₁² + r₁r₂ + r₂²).
    let mut k = kernel();
    for r2 in [5.5, 5.01, 5.000_000_2] {
        let shape = k
            .make(Primitive::Cone {
                radius1: 5.0,
                radius2: r2,
                height: 10.0,
            })
            .unwrap();
        let s = k.summarize(&shape).unwrap();
        assert!(s.valid, "{r2}");
        let volume = PI * 10.0 / 3.0 * (25.0 + 5.0 * r2 + r2 * r2);
        assert!(
            relative_error(s.volume, volume) <= 1e-9,
            "{r2}: {} vs {volume}",
            s.volume
        );
    }
}

#[test]
fn every_primitive_builds_at_the_smallest_and_largest_sizes() {
    // 1.01e-7 (just above OCCT's tolerance) and 1e6 (a kilometre), each primitive alone:
    // analytic volumes. (A ring torus: a horn torus this small is measured only roughly;
    // see docs/ports/primitives.md.)
    let mut k = kernel();
    for r in [1.01e-7, 1e-3, 0.1, 1e6] {
        let cases = [
            (
                Primitive::Box {
                    dx: r,
                    dy: r,
                    dz: r,
                },
                r * r * r,
            ),
            (
                Primitive::Cylinder {
                    radius: r,
                    height: r,
                },
                PI * r * r * r,
            ),
            (Primitive::Sphere { radius: r }, 4.0 / 3.0 * PI * r * r * r),
            (
                Primitive::Cone {
                    radius1: r,
                    radius2: 0.0,
                    height: r,
                },
                PI * r * r * r / 3.0,
            ),
            (
                Primitive::Torus {
                    major_radius: 2.0 * r,
                    minor_radius: r,
                },
                4.0 * PI * PI * r * r * r,
            ),
        ];
        for (primitive, volume) in cases {
            if r == 1e6 && matches!(primitive, Primitive::Torus { .. }) {
                // Its ring, 2r, would be above a kilometre.
                continue;
            }
            let shape = k.make(primitive).unwrap();
            let s = k.summarize(&shape).unwrap();
            assert!(s.valid, "{primitive:?}");
            assert!(
                relative_error(s.volume, volume) <= 1e-6,
                "{primitive:?}: {} vs {volume}",
                s.volume
            );
        }
    }
}

#[test]
fn the_kernel_fillets_every_edge_it_is_given() {
    // KernelWorker::fillet against a real worker: every edge of a 10 × 20 × 30 box at
    // r = 1 is the box shrunk by 2r, grown by a ball of radius r (a Minkowski sum).
    let (a, b, c, r) = (10.0f64, 20.0f64, 30.0f64, 1.0f64);
    let (x, y, z) = (a - 2.0 * r, b - 2.0 * r, c - 2.0 * r);
    let every = x * y * z
        + 2.0 * r * (x * y + y * z + x * z)
        + PI * r * r * (x + y + z)
        + 4.0 / 3.0 * PI * r * r * r;
    let mut k = kernel();
    let shape = k
        .make(Primitive::Box {
            dx: a,
            dy: b,
            dz: c,
        })
        .unwrap();
    let rounded = k
        .fillet(&shape, &(1..=12).collect::<Vec<u32>>(), r)
        .unwrap()
        .shape;
    assert_eq!(rounded.valid(), Some(true));
    let s = k.summarize(&rounded).unwrap();
    assert!(
        relative_error(s.volume, every) <= 1e-6,
        "{} vs {every}",
        s.volume
    );
}

#[test]
fn every_primitive_is_the_same_every_time() {
    // The same bytes twice in one worker and once in another.
    let mut one = kernel();
    let mut other = kernel();
    for primitive in valid_primitives() {
        let a = one.make(primitive).unwrap();
        let b = one.make(primitive).unwrap();
        let c = other.make(primitive).unwrap();
        assert!(a == b && b == c, "{primitive:?}");
    }
    // And the cone with equal radii, built as a cylinder.
    let cylinder = Primitive::Cone {
        radius1: 5.0,
        radius2: 5.0,
        height: 10.0,
    };
    assert!(one.make(cylinder).unwrap() == other.make(cylinder).unwrap());
}

#[test]
fn every_parameter_may_be_a_kilometre_or_just_above_the_tolerance() {
    // The other side of invalid_params_never_reach_the_worker: each parameter of each
    // primitive at exactly the largest length, and at the smallest above the tolerance,
    // builds a valid solid. (A torus's tube can't exceed its ring, so the two move
    // together where needed.)
    let mut k = kernel();
    let pointed = Primitive::Cone {
        radius1: 1.0,
        radius2: 0.0,
        height: 3.0,
    };
    for primitive in valid_primitives().into_iter().chain([pointed]) {
        for &parameter in parameters(primitive) {
            for value in [1e6, 1.01e-7] {
                let p = match (primitive, parameter) {
                    // A tube of a kilometre needs a ring as large; a ring of 1.01e-7 a tube
                    // as small.
                    (Primitive::Torus { .. }, "minor_radius") if value == 1e6 => Primitive::Torus {
                        major_radius: value,
                        minor_radius: value,
                    },
                    (Primitive::Torus { .. }, "major_radius") if value < 1.0 => Primitive::Torus {
                        major_radius: value,
                        minor_radius: value,
                    },
                    _ => with(primitive, parameter, value),
                };
                let shape = k
                    .make(p)
                    .unwrap_or_else(|e| panic!("{parameter} = {value}: {p:?}: {e}"));
                assert_eq!(shape.valid(), Some(true), "{p:?}");
                // Measured right, and with the right faces, edges, and vertices; except a
                // horn torus at the tolerance's scale, which OCCT measures only roughly
                // (docs/ports/primitives.md, Limits).
                let s = k.summarize(&shape).unwrap();
                let (volume, topology) = analytic_volume_and_topology(p);
                assert_eq!((s.faces, s.edges, s.vertices), topology, "{p:?}");
                let horn_at_tolerance =
                    matches!(p, Primitive::Torus { major_radius, .. } if major_radius < 1e-6);
                if !horn_at_tolerance {
                    assert!(
                        relative_error(s.volume, volume) <= 2e-9,
                        "{p:?}: {} vs {volume}",
                        s.volume
                    );
                }
            }
        }
    }
}

#[test]
fn radii_exactly_the_tolerance_apart_make_a_cone() {
    // Within the tolerance means strictly less than 1e-7 apart, as in FreeCAD and OCCT;
    // exactly 1e-7 apart is a cone, with the frustum's volume.
    let (r1, r2, h) = (2e-7f64, 3e-7f64, 1.0f64);
    assert_eq!((r1 - r2).abs(), 1e-7);
    let mut k = kernel();
    let shape = k
        .make(Primitive::Cone {
            radius1: r1,
            radius2: r2,
            height: h,
        })
        .unwrap();
    let s = k.summarize(&shape).unwrap();
    let frustum = PI * h / 3.0 * (r1 * r1 + r1 * r2 + r2 * r2);
    assert!(
        relative_error(s.volume, frustum) <= 1e-6,
        "{} vs {frustum}",
        s.volume
    );
}

#[test]
fn the_kernel_fillets_the_edges_and_radius_it_is_given() {
    // One edge at r = 2, not every edge, not r = 1: the corner of cross-section r² − πr²/4
    // along Edge1's 30 is removed, and one face, three edges, and two vertices are added.
    let mut k = kernel();
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let rounded = k.fillet(&shape, &[1], 2.0).unwrap().shape;
    let s = k.summarize(&rounded).unwrap();
    let volume = 6000.0 - (4.0 - PI) * 30.0;
    assert!(
        relative_error(s.volume, volume) <= 1e-12,
        "{} vs {volume}",
        s.volume
    );
    assert_eq!((s.faces, s.edges, s.vertices), (7, 15, 10));
    // Which edge: the removed corner's centroid, from the corner, is r³(5/6 − π/4)/A along
    // x and y, where A = r² − πr²/4; so the center moves away from the edge at the origin,
    // which is Edge1 (FreeCAD's numbering). Edges 3, 5, and 7 have the same volume.
    let (r, length) = (2.0f64, 30.0);
    let area = r * r - PI * r * r / 4.0;
    let from_corner = r * r * r * (5.0 / 6.0 - PI / 4.0) / area;
    let removed = area * length;
    let center = [
        (6000.0 * 5.0 - removed * from_corner) / volume,
        (6000.0 * 10.0 - removed * from_corner) / volume,
        15.0,
    ];
    for (got, want) in s.center.iter().zip(center) {
        assert!(
            (got - want).abs() <= 1e-9,
            "center {:?} vs {center:?}",
            s.center
        );
    }
}

/// A primitive's volume from its formula, and its faces, edges, and vertices.
fn analytic_volume_and_topology(p: Primitive) -> (f64, (u32, u32, u32)) {
    match p {
        Primitive::Box { dx, dy, dz } => (dx * dy * dz, (6, 12, 8)),
        Primitive::Cylinder { radius, height } => (PI * radius * radius * height, (3, 3, 2)),
        Primitive::Sphere { radius } => (4.0 / 3.0 * PI * radius * radius * radius, (1, 3, 2)),
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => {
            let volume =
                PI * height / 3.0 * (radius1 * radius1 + radius1 * radius2 + radius2 * radius2);
            // A pointed cone has no face at its point.
            let faces = if radius1 == 0.0 || radius2 == 0.0 {
                2
            } else {
                3
            };
            (volume, (faces, 3, 2))
        }
        Primitive::Torus {
            major_radius,
            minor_radius,
        } => (
            2.0 * PI * PI * major_radius * minor_radius * minor_radius,
            (1, 2, 1),
        ),
    }
}

#[test]
fn refusals_show_the_value_given() {
    // The value refused, readable: as written, or in exponent form when very large or
    // very small.
    let mut k = kernel();
    let cases = [
        (Primitive::Sphere { radius: -1.0 }, "got -1"),
        (Primitive::Sphere { radius: 1e-7 }, "got 1e-7"),
        (Primitive::Sphere { radius: 1e12 }, "got 1e12"),
        (
            Primitive::Sphere { radius: f64::MAX },
            "got 1.7976931348623157e308",
        ),
        (Primitive::Sphere { radius: f64::NAN }, "got NaN"),
        (
            Primitive::Box {
                dx: 0.0,
                dy: 1.0,
                dz: 1.0,
            },
            "got 0",
        ),
        (
            Primitive::Cone {
                radius1: 2e6,
                radius2: 1.0,
                height: 1.0,
            },
            "got 2000000",
        ),
        (
            Primitive::Torus {
                major_radius: 10.0,
                minor_radius: 12.0,
            },
            "major_radius (10), got 12:",
        ),
    ];
    for (p, shows) in cases {
        let err = k.make(p).unwrap_err();
        assert!(err.message().contains(shows), "{p:?}: {err}");
    }
}

/// A box's B-rep with every `30.0` in the section that starts at `section` (up to the
/// next section) replaced by `with`, and a checksum that matches: as a badly stored shape
/// might be. Also returns how many values were replaced.
fn spoiled_box(k: &mut KernelWorker, section: &[u8], with: f64) -> (ShapeData, usize) {
    let shape = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let mut bytes = shape.brep().as_bytes().to_vec();
    bytes.truncate(bytes.len() - 4);
    let find = |name: &[u8], from: usize| {
        bytes[from..]
            .windows(name.len())
            .position(|w| w == name)
            .map(|at| from + at)
    };
    let start = find(section, 0).unwrap();
    let end = [
        &b"Curves"[..],
        b"Polygon3D",
        b"Surfaces",
        b"Triangulations",
        b"TShapes",
    ]
    .into_iter()
    .filter_map(|name| find(name, start + section.len()))
    .min()
    .unwrap();
    let (from, to) = (30.0f64.to_le_bytes(), with.to_le_bytes());
    let (mut i, mut replaced) = (start, 0);
    while i + 8 <= end {
        if bytes[i..i + 8] == from {
            bytes[i..i + 8].copy_from_slice(&to);
            replaced += 1;
            i += 8;
        } else {
            i += 1;
        }
    }
    let crc = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    (ShapeData::unchecked(Blob::inline(bytes)), replaced)
}

#[test]
fn a_shape_with_a_curve_that_isnt_finite_doesnt_cost_a_worker() {
    // NaN in the box's curves: OCCT meshes it into coordinates that aren't finite. That's
    // the shape's fault, reported by the worker, which carries on; a broken worker is a
    // different error.
    let mut k = kernel();
    let (bad, replaced) = spoiled_box(&mut k, b"Curves", f64::NAN);
    assert!(replaced > 0);
    let err = k.tessellate(&bad, 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert!(
        err.cause()
            .unwrap()
            .contains("coordinates that aren't finite"),
        "{err}"
    );
    assert!(err.suggestion().unwrap().contains("rebuild"), "{err}");
    // The worker is still there.
    let good = k
        .make(Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        })
        .unwrap();
    k.summarize(&good).unwrap();
}

#[test]
fn a_shape_whose_extent_isnt_finite_is_the_shapes_fault() {
    // Infinity in the curves: the shape's bounding box isn't finite, so no deflection
    // would do. The error says so, rather than asking for an infinite deflection.
    let mut k = kernel();
    let (bad, replaced) = spoiled_box(&mut k, b"Curves", f64::INFINITY);
    assert!(replaced > 0);
    let err = k.tessellate(&bad, 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert!(
        err.cause()
            .unwrap()
            .contains("extent is too large or not finite"),
        "{err}"
    );
    assert!(err.suggestion().unwrap().contains("rebuild"), "{err}");
}

#[test]
fn a_shape_with_a_surface_that_isnt_finite_suggests_rebuilding_it() {
    // NaN in the box's planes: OCCT can't mesh those faces. The suggestion is about the
    // shape, not the deflection, which no change would help.
    let mut k = kernel();
    let (bad, replaced) = spoiled_box(&mut k, b"Surfaces", f64::NAN);
    assert!(replaced > 0);
    let err = k.tessellate(&bad, 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert!(err.suggestion().unwrap().contains("rebuild"), "{err}");
}
