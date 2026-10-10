//! M1.5: extrude and revolve (docs/ports/extrude-revolve.md), checked against hand
//! calculations, FreeCAD's own tests, FreeCAD's element names, the geometry of the ends,
//! and through a worker.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/close.rs"]
mod close;
#[path = "common/facts.rs"]
mod facts;
#[path = "common/imports.rs"]
mod imports;
#[path = "common/numbers.rs"]
mod numbers;
#[path = "common/oracle.rs"]
mod oracle;

use std::f64::consts::PI;
use std::path::Path;
use std::time::Duration;

use close::assert_close;
use facts::{Fact, facts};
use silico_core::ErrorCode;
use silico_core::kernel::KernelWorker;
use silico_core::worker::Worker;
use silico_kernel::{
    Axis, Built, ElementKind, KernelErrorKind, Profile, Shape, ShapeSummary, extrude, revolve,
    summarize, tessellate,
};
use silico_protocol::{DebugOp, Generation, Misbehavior, Op};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

fn profile(points: &[[f64; 3]]) -> Profile {
    Profile {
        points: points.to_vec(),
    }
}

/// The `dx` × `dy` rectangle in the XY plane, counterclockwise from the origin (its
/// normal is +z).
fn rectangle(dx: f64, dy: f64) -> Profile {
    profile(&[
        [0.0, 0.0, 0.0],
        [dx, 0.0, 0.0],
        [dx, dy, 0.0],
        [0.0, dy, 0.0],
    ])
}

/// FreeCAD's revolve test profile: `width` from x = `x0` and `height` up z, in the XZ
/// plane.
fn upright(x0: f64, width: f64, height: f64) -> Profile {
    profile(&[
        [x0, 0.0, 0.0],
        [x0 + width, 0.0, 0.0],
        [x0 + width, 0.0, height],
        [x0, 0.0, height],
    ])
}

fn axis(origin: [f64; 3], direction: [f64; 3]) -> Axis {
    Axis { origin, direction }
}

const Z: [f64; 3] = [0.0, 0.0, 1.0];

fn summary(built: &Built) -> ShapeSummary {
    summarize(&built.shape).unwrap()
}

/// The mesh vertices of each face of `shape` (fine enough to show where a face is).
fn face_points(shape: &Shape) -> Vec<Vec<[f64; 3]>> {
    let mesh = tessellate(shape, 0.05).unwrap();
    let mut out = Vec::new();
    let mut first = 0usize;
    for &count in &mesh.face_triangles {
        let triangles = &mesh.triangles[first..first + count as usize];
        out.push(
            triangles
                .iter()
                .flat_map(|t| t.iter().map(|&i| mesh.positions[i as usize]))
                .collect(),
        );
        first += count as usize;
    }
    out
}

/// The box around a fine mesh of `shape`: its vertices lie on the surface, so the box is
/// within the deflection (0.002) of the shape's own.
fn mesh_box(shape: &Shape) -> [[f64; 3]; 2] {
    let mesh = tessellate(shape, 0.002).unwrap();
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &mesh.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    [lo, hi]
}

/// `shape`'s box is `want` (min corner, max corner), to the mesh's deflection.
fn assert_box(what: &str, shape: &Shape, want: [[f64; 3]; 2]) {
    let got = mesh_box(shape);
    for (g, w) in got.iter().flatten().zip(want.iter().flatten()) {
        assert!((g - w).abs() <= 0.003, "{what}: box {got:?}, want {want:?}");
    }
}

#[test]
fn extrude_rect_and_its_ends_are_where_the_geometry_says() {
    // From the roadmap: a 10 × 20 rectangle extruded 5 is 1000, a box: 6 faces, 12 edges,
    // 8 vertices, area 2 × (200 + 50 + 100), center (5, 10, 2.5). Its start is the face at
    // z = 0, its end the face at z = 5.
    let built = extrude(&rectangle(10.0, 20.0), 5.0, None).unwrap();
    let s = summary(&built);
    assert_close("volume", s.volume, 1000.0, 1e-12);
    assert_close("area", s.area, 700.0, 1e-12);
    for (got, want) in s.center.into_iter().zip([5.0, 10.0, 2.5]) {
        assert_close("center", got, want, 1e-12);
    }
    assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8));
    assert!(s.valid);

    let h = &built.history;
    assert_eq!(h.validate(1), Ok(()));
    assert_eq!(
        (h.ends.start.len(), h.ends.end.len()),
        (1, 1),
        "{:?}",
        h.ends
    );
    let faces = face_points(&built.shape);
    let at_z = |face: u32, z: f64| {
        faces[face as usize - 1]
            .iter()
            .all(|p| (p[2] - z).abs() < 1e-9)
    };
    assert!(at_z(h.ends.start[0], 0.0), "start {:?}", h.ends);
    assert!(at_z(h.ends.end[0], 5.0), "end {:?}", h.ends);
    // The profile's face is the bottom, kept, and generates the solid.
    let face = &h.inputs[0].faces[0];
    assert_eq!(face.kept, Some(h.ends.start[0]));
    assert_eq!(face.generated.len(), 1);
    assert_eq!(face.generated[0].element.kind, ElementKind::Solid);
}

#[test]
fn each_side_and_corner_of_the_profile_makes_a_side_face_and_edge() {
    // Edge n of the profile runs from point n to point n + 1, and stays as an edge of the
    // bottom; it generates the side face over it. Vertex n is point n, stays, and
    // generates the upright edge above it.
    let built = extrude(&rectangle(10.0, 20.0), 5.0, None).unwrap();
    let h = &built.history;
    let input = &h.inputs[0];
    assert_eq!(
        (input.faces.len(), input.edges.len(), input.vertices.len()),
        (1, 4, 4)
    );
    let faces = face_points(&built.shape);
    let points = rectangle(10.0, 20.0).points;
    for (n, edge) in input.edges.iter().enumerate() {
        assert!(edge.kept.is_some(), "Edge{}: {edge:?}", n + 1);
        assert_eq!(edge.generated.len(), 1, "Edge{}: {edge:?}", n + 1);
        let side = edge.generated[0].element;
        assert_eq!(side.kind, ElementKind::Face);
        // The side face lies over the edge: every point of it is on the vertical plane
        // through the edge's two points.
        let (a, b) = (points[n], points[(n + 1) % 4]);
        let on_plane = |p: &[f64; 3]| {
            let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            cross.abs() < 1e-9
        };
        assert!(
            faces[side.index as usize - 1].iter().all(on_plane),
            "Edge{}'s face",
            n + 1
        );
    }
    for (n, vertex) in input.vertices.iter().enumerate() {
        assert!(vertex.kept.is_some(), "Vertex{}: {vertex:?}", n + 1);
        assert_eq!(vertex.generated.len(), 1, "Vertex{}", n + 1);
        assert_eq!(vertex.generated[0].element.kind, ElementKind::Edge);
    }
}

#[test]
fn an_extrusion_goes_the_way_the_profile_faces_or_the_direction_points() {
    // The same rectangle with its corners the other way faces down, and goes down; along
    // (1, 0, 1) it leans, still rising 5 (a slanted box of the same volume, its center
    // moved 2.5 along x); along (0, 0, -2) it goes down 5.
    let mut down = rectangle(10.0, 20.0);
    down.points.reverse();
    let cases = [
        (down, None, [5.0, 10.0, -2.5]),
        (
            rectangle(10.0, 20.0),
            Some([1.0, 0.0, 1.0]),
            [7.5, 10.0, 2.5],
        ),
        (
            rectangle(10.0, 20.0),
            Some([0.0, 0.0, -2.0]),
            [5.0, 10.0, -2.5],
        ),
    ];
    for (p, direction, center) in cases {
        let s = summary(&extrude(&p, 5.0, direction).unwrap());
        assert_close(&format!("{direction:?} volume"), s.volume, 1000.0, 1e-12);
        for (got, want) in s.center.into_iter().zip(center) {
            assert_close(&format!("{direction:?} center"), got, want, 1e-12);
        }
        assert!(s.valid);
    }
}

#[test]
fn revolve_rings_full_and_quarter() {
    // From the roadmap: a 2 × 10 rectangle from x = 3 to 5, revolved about z: a tube of
    // π (5² − 3²) × 10 = 160π, and a quarter of it, 40π.
    let ring = upright(3.0, 2.0, 10.0);
    let full = revolve(&ring, &axis([0.0; 3], Z), 360.0).unwrap();
    let s = summary(&full);
    assert_close("full", s.volume, 160.0 * PI, 1e-9);
    // Outer and inner walls, top and bottom: 2π(5 + 3) × 10 + 2 × π(25 − 9).
    assert_close("full area", s.area, 160.0 * PI + 32.0 * PI, 1e-9);
    assert!(s.valid);
    // A full turn has no ends, and the profile's face isn't in it.
    assert_eq!(full.history.ends, silico_kernel::Ends::default());
    assert!(full.history.inputs[0].faces[0].is_deleted());

    let quarter = revolve(&ring, &axis([0.0; 3], Z), 90.0).unwrap();
    let s = summary(&quarter);
    assert_close("quarter", s.volume, 40.0 * PI, 1e-9);
    // About +z, x turns toward +y.
    assert_box(
        "quarter",
        &quarter.shape,
        [[0.0, 0.0, 0.0], [5.0, 5.0, 10.0]],
    );
    assert!(s.valid);
    // It starts at the profile (in the XZ plane, y = 0) and ends a quarter turn on, in the
    // YZ plane (x = 0).
    let h = &quarter.history;
    let faces = face_points(&quarter.shape);
    let flat = |face: u32, coordinate: usize| {
        faces[face as usize - 1]
            .iter()
            .all(|p| p[coordinate].abs() < 1e-9)
    };
    assert_eq!(
        (h.ends.start.len(), h.ends.end.len()),
        (1, 1),
        "{:?}",
        h.ends
    );
    assert!(flat(h.ends.start[0], 1), "start {:?}", h.ends);
    assert!(flat(h.ends.end[0], 0), "end {:?}", h.ends);
    assert_eq!(h.inputs[0].faces[0].kept, Some(h.ends.start[0]));
}

#[test]
fn freecads_extrusion_and_revolution_tests() {
    // tests/src/Mod/Part/App/FeatureExtrusion.cpp: a 3 × 4 rectangle extruded 10 is 120,
    // forward or reversed.
    let rect = rectangle(3.0, 4.0);
    assert_close(
        "forward",
        summary(&extrude(&rect, 10.0, None).unwrap()).volume,
        120.0,
        1e-12,
    );
    let forward = extrude(&rect, 10.0, None).unwrap();
    assert_box(
        "forward",
        &forward.shape,
        [[0.0, 0.0, 0.0], [3.0, 4.0, 10.0]],
    );
    let back = extrude(&rect, 10.0, Some([0.0, 0.0, -1.0])).unwrap();
    assert_close("reversed", summary(&back).volume, 120.0, 1e-12);
    assert_box(
        "reversed",
        &back.shape,
        [[0.0, 0.0, -10.0], [3.0, 4.0, 0.0]],
    );
    // testExecuteDir: along (0, 1, 1), 10 along the direction, so a rise of 10 sin 45°
    // along the normal: 120 sin 45°, leaning toward +y.
    let sin45 = std::f64::consts::FRAC_1_SQRT_2;
    let dir = extrude(&rect, 10.0 * sin45, Some([0.0, 1.0, 1.0])).unwrap();
    assert_close("Dir", summary(&dir).volume, 120.0 * sin45, 1e-12);
    assert_box(
        "Dir",
        &dir.shape,
        [[0.0, 0.0, 0.0], [3.0, 4.0 + 10.0 * sin45, 10.0 * sin45]],
    );
    // tests/src/Mod/Part/App/FeatureRevolution.cpp: the same rectangle (x 0..3, y 0..4)
    // about y: a puck of radius 3 and height 4; with the axis at x = 4: radii 1 to 4; about
    // x: radius 4, height 3; by 90°: a quarter puck.
    // Their boxes pin which way a turn goes: by 90° about +y, x turns toward −z.
    let y = [0.0, 1.0, 0.0];
    let cases = [
        (
            axis([0.0; 3], y),
            360.0,
            PI * 9.0 * 4.0,
            [[-3.0, 0.0, -3.0], [3.0, 4.0, 3.0]],
        ),
        (
            axis([4.0, 0.0, 0.0], y),
            360.0,
            PI * (16.0 - 1.0) * 4.0,
            [[0.0, 0.0, -4.0], [8.0, 4.0, 4.0]],
        ),
        (
            axis([0.0; 3], [1.0, 0.0, 0.0]),
            360.0,
            PI * 16.0 * 3.0,
            [[0.0, -4.0, -4.0], [3.0, 4.0, 4.0]],
        ),
        (
            axis([0.0; 3], y),
            90.0,
            PI * 9.0 * 4.0 / 4.0,
            [[0.0, 0.0, -3.0], [3.0, 4.0, 0.0]],
        ),
    ];
    for (a, angle, volume, bounds) in cases {
        let built = revolve(&rect, &a, angle).unwrap();
        let s = summary(&built);
        assert_close(&format!("{a:?} by {angle}"), s.volume, volume, 1e-9);
        assert!(s.valid, "{a:?}");
        assert_box(&format!("{a:?} by {angle}"), &built.shape, bounds);
    }
    // tests/src/Mod/Part/App/TopoShapeExpansion.cpp makeElementPrism: a unit cube's Face1
    // (x = 0) extruded 0.75 along x is 0.75. makeElementRevolve: the same face about y by
    // 45, which FreeCAD passes as radians and OCCT folds to 45 − 14π (58.31°): 0.50885141
    // (Pappus: area 1, centroid 0.5 from the axis).
    let face = profile(&[
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [0.0, 0.0, 1.0],
    ]);
    let prism = extrude(&face, 0.75, Some([1.0, 0.0, 0.0])).unwrap();
    assert_close("prism", summary(&prism).volume, 0.75, 1e-12);
    let folded = (45.0 - 14.0 * PI) * (180.0 / PI);
    let swept = revolve(&face, &axis([0.0; 3], y), folded).unwrap();
    assert_close("revolve", summary(&swept).volume, 0.508_851_41, 1e-7);
    assert_box(
        "revolve",
        &swept.shape,
        [[0.0, 0.0, 0.0], [0.850_903_524_534_119_3, 1.0, 1.0]],
    );
    assert_close(
        "Pappus",
        summary(&swept).volume,
        0.5 * (45.0 - 14.0 * PI),
        1e-12,
    );
}

#[test]
fn a_profile_touching_the_axis_revolves_into_a_solid_without_a_hole() {
    // A 3 × 4 rectangle with a side on the axis: a solid cylinder, 36π, three faces (side,
    // top, bottom).
    let s = summary(&revolve(&upright(0.0, 3.0, 4.0), &axis([0.0; 3], Z), 360.0).unwrap());
    assert_close("puck", s.volume, 36.0 * PI, 1e-9);
    assert_eq!(s.faces, 3);
    assert!(s.valid);
}

#[test]
fn concave_triangular_and_tilted_profiles_sweep() {
    // An L (a 2 × 2 square less a 1 × 1 corner: 3) extruded 2 is 6; a right triangle with
    // legs 3 and 4 extruded 1 is 6; a unit square tilted in the plane x + z = 0 extruded 1
    // along its normal is 1 (its normal is (1, 0, 1)/√2).
    let l = profile(&[
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 2.0, 0.0],
        [0.0, 2.0, 0.0],
    ]);
    let triangle = profile(&[[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [0.0, 4.0, 0.0]]);
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let tilted = profile(&[[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-h, 1.0, h], [-h, 0.0, h]]);
    for (what, p, length, volume) in [
        ("L", l, 2.0, 6.0),
        ("triangle", triangle, 1.0, 6.0),
        ("tilted", tilted, 1.0, 1.0),
    ] {
        let built = extrude(&p, length, None).unwrap();
        let s = summary(&built);
        assert_close(what, s.volume, volume, 1e-9);
        assert!(s.valid, "{what}");
        assert_eq!(built.history.validate(1), Ok(()), "{what}");
    }
}

/// How a case sweeps: along a vector, or around an axis by an angle in degrees.
#[derive(Debug, Clone, Copy)]
enum Sweep {
    Extrude([f64; 3]),
    Revolve(Axis, f64),
}

/// What FreeCAD makes of sweeping the polygon through `points` (Python's `extrude` and
/// `revolve`, on a document feature's face so its elements are named): volume, area,
/// counts, validity, the history its element names state (read as boolean.rs reads them),
/// and the faces it names as the extrusion's far end (`;:G0`, a face parallel to the
/// profile).
fn freecad_sweep(points: &[[f64; 3]], sweep: Sweep) -> serde_json::Value {
    let call = match sweep {
        Sweep::Extrude(v) => format!("extrude(FreeCAD.Vector({}, {}, {}))", v[0], v[1], v[2]),
        Sweep::Revolve(a, angle) => format!(
            "revolve(FreeCAD.Vector({}, {}, {}), FreeCAD.Vector({}, {}, {}), {angle:?})",
            a.origin[0], a.origin[1], a.origin[2], a.direction[0], a.direction[1], a.direction[2]
        ),
    };
    let script = format!(
        r#"
import json, re, FreeCAD, Part
doc = FreeCAD.newDocument()
f = doc.addObject("Part::Feature", "Profile")
f.Shape = Part.Face(Part.makePolygon([FreeCAD.Vector(*p) for p in {points:?}], True))
doc.recompute()
r = f.Shape.{call}
tag = format(f.Shape.Tag, "x")
EL = r"([A-Za-z]+[0-9]+)"
kept = re.compile(EL + r";:H([0-9a-f]+),[FEV]")
made = re.compile(EL + r";:([MG])([0-9]*)(?:\((.*)\))?;[A-Z]+;:H([0-9a-f]+):[0-9a-f]+,[FEV]")
facts, ends, unread = [], [], []
for result, name in r.ElementReverseMap.items():
    if ";:U" in name or ";:L" in name:
        continue
    m = kept.fullmatch(name)
    if m and m[2] == tag:
        facts.append(["kept", m[1], result, 0])
        continue
    m = made.fullmatch(name)
    if not m or m[5] != tag or m[4]:
        unread.append(name)
        continue
    if m[3] == "0":
        ends.append(result)
        continue
    facts.append(["modified" if m[2] == "M" else "generated", m[1], result, int(m[3] or 1)])
out = {{"volume": r.Volume, "area": r.Area, "center": list(r.CenterOfMass), "faces": len(r.Faces), "edges": len(r.Edges),
        "vertices": len(r.Vertexes), "valid": r.isValid(), "facts": facts, "ends": ends,
        "unread": unread}}
print("SILICO_ORACLE " + json.dumps(out))
"#
    );
    oracle::freecad(&script)
}

#[test]
fn sweeps_match_freecad_with_the_history_its_names_state() {
    let tilted_l = [
        [0.0, 0.0, 0.0],
        [4.0, 0.0, 1.0],
        [4.0, 2.0, 1.0],
        [2.0, 2.0, 0.5],
        [2.0, 5.0, 0.5],
        [0.0, 5.0, 0.0],
    ];
    let triangle = [[1.0, 0.0, 0.0], [4.0, 0.0, 0.0], [2.0, 0.0, 3.0]];
    let cases: [(&[[f64; 3]], Sweep); 6] = [
        (
            &rectangle(10.0, 20.0).points,
            Sweep::Extrude([0.0, 0.0, 5.0]),
        ),
        (&tilted_l, Sweep::Extrude([1.0, -2.0, 7.0])),
        (&triangle, Sweep::Extrude([0.0, 3.0, 0.0])),
        (
            &upright(3.0, 2.0, 10.0).points,
            Sweep::Revolve(axis([0.0; 3], Z), 90.0),
        ),
        (
            &upright(3.0, 2.0, 10.0).points,
            Sweep::Revolve(axis([0.0; 3], Z), 360.0),
        ),
        (&triangle, Sweep::Revolve(axis([0.0, 0.0, 0.0], Z), 200.0)),
    ];
    for (points, sweep) in cases {
        let case = format!("{sweep:?} of {points:?}");
        let theirs = freecad_sweep(points, sweep);
        assert_eq!(theirs["unread"], serde_json::json!([]), "{case}");
        let p = profile(points);
        let built = match sweep {
            Sweep::Extrude(v) => {
                // FreeCAD sweeps along the vector; Silico takes the length along the
                // normal, so give it the vector's rise.
                let normal = p.check().unwrap().normal;
                let rise = v.iter().zip(normal).map(|(a, b)| a * b).sum::<f64>();
                extrude(&p, rise.abs(), Some(v)).unwrap()
            }
            Sweep::Revolve(a, angle) => revolve(&p, &a, angle).unwrap(),
        };
        let s = summary(&built);
        // FreeCAD measures with OCCT's fixed order of points, Silico adaptively.
        assert_close(&case, s.volume, theirs["volume"].as_f64().unwrap(), 1e-7);
        assert_close(&case, s.area, theirs["area"].as_f64().unwrap(), 1e-7);
        // Where it is, so a sweep the wrong way round (the same volume, area, and history)
        // can't pass.
        for (ours, theirs) in s.center.iter().zip(theirs["center"].as_array().unwrap()) {
            let theirs = theirs.as_f64().unwrap();
            assert!(
                (ours - theirs).abs() <= 1e-6,
                "{case}: center {:?} vs {theirs}",
                s.center
            );
        }
        let count = |key: &str| theirs[key].as_u64().unwrap() as u32;
        assert_eq!(
            (s.faces, s.edges, s.vertices),
            (count("faces"), count("edges"), count("vertices")),
            "{case}"
        );
        assert_eq!(s.valid, theirs["valid"].as_bool().unwrap(), "{case}");

        let ours = facts(&built.history);
        let mut stated: Vec<Fact> = theirs["facts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let s = |i: usize| f[i].as_str().unwrap().to_owned();
                (s(0), s(1), s(2), f[3].as_u64().unwrap() as u32)
            })
            .collect();
        stated.sort();
        assert!(stated.len() >= 4, "{case}: FreeCAD stated only {stated:?}");
        for fact in &stated {
            assert!(
                ours.contains(fact),
                "{case}: FreeCAD has {fact:?}; Silico has {ours:?}"
            );
        }
        let kept =
            |fs: &[Fact]| -> Vec<Fact> { fs.iter().filter(|f| f.0 == "kept").cloned().collect() };
        assert_eq!(kept(&ours), kept(&stated), "{case}: kept");
        // The face FreeCAD guesses is the extrusion's far end is the one Silico reports.
        for end in theirs["ends"].as_array().unwrap() {
            let face: u32 = end
                .as_str()
                .unwrap()
                .strip_prefix("Face")
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(built.history.ends.end, [face], "{case}");
        }
    }
}

#[test]
fn values_that_cant_make_a_solid_are_refused_naming_what() {
    let rect = rectangle(1.0, 1.0);
    let refused = |result: Result<Built, silico_kernel::KernelError>, says: &str| {
        let err = result.err().unwrap();
        assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{err}");
        assert!(err.message().contains(says), "{err}");
    };
    refused(
        extrude(&profile(&[[0.0; 3], [1.0, 0.0, 0.0]]), 1.0, None),
        "corners",
    );
    refused(extrude(&rect, 0.0, None), "length");
    refused(
        extrude(&rect, 1.0, Some([1.0, 0.0, 0.0])),
        "lies in the profile's plane",
    );
    let upright = upright(1.0, 1.0, 1.0);
    refused(
        revolve(&upright, &axis([0.0; 3], [0.0, 1.0, 0.0]), 90.0),
        "must lie in the profile's plane",
    );
    refused(
        revolve(&upright, &axis([1.5, 0.0, 0.0], Z), 90.0),
        "crosses the profile",
    );
    refused(revolve(&upright, &axis([0.0; 3], Z), 0.0), "angle");
    refused(revolve(&upright, &axis([0.0; 3], Z), 400.0), "angle");
}

#[test]
fn a_profile_flat_within_the_tolerance_turns_about_its_own_side() {
    // A unit square with a corner 9e-8 off flat, within the
    // tolerance, revolved a quarter turn about its own side from (0, 0, 0) to
    // (0, 1, 9e-8): the axis is within the tolerance of the profile's plane wherever it
    // passes it, so it's in the plane. A quarter of a puck of radius 1 and height 1: π/4.
    let bent = profile(&[
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 9e-8],
    ]);
    let built = revolve(&bent, &axis([0.0; 3], [0.0, 1.0, 9e-8]), 90.0)
        .unwrap_or_else(|e| panic!("refused: {e}"));
    let s = summary(&built);
    assert_close("bent", s.volume, PI / 4.0, 1e-6);
    // A quarter puck, as the flat square makes, not a tube around a sliver of a hole.
    assert!(s.valid);
    assert_eq!((s.faces, s.edges, s.vertices), (5, 9, 6));
    assert_box("bent", &built.shape, [[0.0, 0.0, -1.0], [1.0, 1.0, 0.0]]);
}

#[test]
fn a_negative_angle_is_refused_saying_to_reverse_the_axis() {
    let err = revolve(&upright(1.0, 1.0, 1.0), &axis([0.0; 3], Z), -90.0)
        .err()
        .unwrap();
    assert_eq!(err.kind(), KernelErrorKind::InvalidInput, "{err}");
    assert!(
        err.message().contains("reverse the axis's direction"),
        "{err}"
    );
}

#[test]
fn sweeps_through_the_worker_match_in_process_and_refusals_never_reach_it() {
    let mut k = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let rect = rectangle(10.0, 20.0);
    let built = k.extrude(&rect, 5.0, None).unwrap();
    let here = extrude(&rect, 5.0, None).unwrap();
    assert_eq!(
        built.shape.brep().as_bytes(),
        silico_kernel::to_brep(&here.shape).unwrap().as_slice()
    );
    assert_eq!(built.history, here.history);
    assert_eq!(built.shape.valid(), Some(true));
    let ring = upright(3.0, 2.0, 10.0);
    let turned = k.revolve(&ring, axis([0.0; 3], Z), 90.0).unwrap();
    assert_eq!(
        turned.history,
        revolve(&ring, &axis([0.0; 3], Z), 90.0).unwrap().history
    );
    assert_close(
        "quarter",
        k.summarize(&turned.shape).unwrap().volume,
        40.0 * PI,
        1e-9,
    );

    let sent = k.requests_sent();
    let crossed = profile(&[[0.0; 3], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    let err = k.extrude(&crossed, 1.0, None).unwrap_err();
    assert_eq!(err.suggestion(), Some("give the extrusion a valid profile"));
    for err in [
        k.extrude(&rect, f64::NAN, None).unwrap_err(),
        k.extrude(&rect, 1.0, Some([0.0, 1.0, 0.0])).unwrap_err(),
        k.extrude(&profile(&[[0.0; 3]; 3]), 1.0, None).unwrap_err(),
        k.revolve(&ring, axis([4.0, 0.0, 0.0], Z), 90.0)
            .unwrap_err(),
        k.revolve(&ring, axis([0.0; 3], Z), -1.0).unwrap_err(),
    ] {
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        assert!(err.suggestion().is_some(), "{err}");
    }
    assert_eq!(k.requests_sent(), sent);
}

#[test]
fn the_worker_refuses_bad_values_itself_and_goes_on() {
    // Sent straight to the worker, past the core's checks.
    let mut w = Worker::spawn(Path::new(WORKER)).unwrap();
    let timeout = Duration::from_secs(30);
    let bad = [
        Op::Extrude {
            profile: rectangle(1.0, 1.0),
            length: -1.0,
            direction: None,
        },
        Op::Revolve {
            profile: upright(1.0, 1.0, 1.0),
            axis: axis([0.0; 3], [0.0, 1.0, 0.0]),
            angle: 90.0,
        },
    ];
    for op in bad {
        let err = w.call(Generation(1), op, timeout).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        let good = Op::Extrude {
            profile: rectangle(1.0, 1.0),
            length: 1.0,
            direction: None,
        };
        assert!(w.call(Generation(1), good, timeout).is_ok());
    }
}

#[test]
fn a_sweep_history_for_the_wrong_number_of_inputs_is_refused() {
    let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    w.call(
        Generation(0),
        Op::Debug(DebugOp::Misbehave(Misbehavior::HistoryForNoInputs)),
        Duration::from_secs(30),
    )
    .unwrap();
    let mut k = KernelWorker::from_worker(w);
    let err = k.extrude(&rectangle(1.0, 1.0), 1.0, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
}

#[test]
fn the_same_sweep_twice_gives_the_same_bytes_and_history() {
    let tilted = profile(&[
        [0.0, 0.0, 0.0],
        [4.0, 0.0, 1.0],
        [4.0, 2.0, 1.0],
        [0.0, 2.0, 0.0],
    ]);
    let builds: [fn(&Profile) -> Built; 2] = [
        |p| extrude(p, 3.0, Some([1.0, 1.0, 5.0])).unwrap(),
        |p| revolve(p, &axis([0.0; 3], [1.0, 0.0, 0.25]), 135.0).unwrap(),
    ];
    for build in builds {
        let (a, b) = (build(&tilted), build(&tilted));
        assert_eq!(
            silico_kernel::to_brep(&a.shape).unwrap(),
            silico_kernel::to_brep(&b.shape).unwrap()
        );
        assert_eq!(a.history, b.history);
    }
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
