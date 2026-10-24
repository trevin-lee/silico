//! break-it: hostile and degenerate shapes for `describe` (docs/design/m4.4-describe.md):
//! mirrored geometry, INTERNAL and EXTERNAL faces, slanted cuts that make ellipses, partial
//! revolves, cones, tori, and boxes at the ends of the length range, random planar solids
//! checked by the divergence theorem, and garbage sent to a worker. Every expected value is
//! a closed form or FreeCAD's (named where it's used).

use super::close;
use super::numbers;

use std::f64::consts::{FRAC_PI_2, PI};
use std::path::Path;

use close::assert_close;
use numbers::relative_error;
use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::kernel::KernelWorker;
use silico_kernel::{
    Axis, BooleanOp, ChamferSize, Primitive, Profile, Shape, boolean, chamfer, describe_whole,
    extrude, from_brep, from_text_brep, make, revolve, summarize, translate,
};
use silico_protocol::{Blob, Description, EdgeCurve, ShapeData, Surface};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/brep")
        .join(name);
    std::fs::read(path).unwrap()
}

fn with_crc(mut body: Vec<u8>) -> Vec<u8> {
    body.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
    body
}

fn described(shape: &Shape) -> Description {
    let d = describe_whole(shape).unwrap();
    d.validate(1).unwrap();
    d
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: [f64; 3]) -> f64 {
    libm::sqrt(dot(a, a))
}

fn assert_point(what: &str, got: [f64; 3], want: [f64; 3], tolerance: f64) {
    let off = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0.0, f64::max);
    assert!(off <= tolerance, "{what}: got {got:?}, want {want:?}");
}

// ---- Mirrored geometry: a plane whose axes are left-handed ----

#[test]
fn a_mirrored_boxs_planes_face_out_of_it() {
    // A 10 × 20 × 30 box mirrored in the plane x = 0 by FreeCAD (`Shape.mirror`, 1.1.3 on
    // OCCT 7.9.3), written as a text B-rep: its planes' axes are left-handed (gp_Ax3
    // indirect: the file's plane 1 has N = −x, X = +z, Y = −y, and N × X = +y), so each
    // plane's normal, Du × Dv, is the opposite of its axis's direction. The box spans
    // x ∈ [−10, 0]; FreeCAD's `Face.normalAt` on its faces gives +x, −x, −y, +y, −z, +z
    // (Face1 at x = 0, Forward, axis −x, normal +x). A point of each face moved 0.01
    // along its normal must be outside the box, and against it inside.
    //
    // Broke before #121's fix: describe takes the plane's axis direction as its normal and reverses
    // it only
    // for a REVERSED face; for a left-handed plane that's the inward normal, so every face
    // of the mirrored box is said to face into it (Face1's normal is −x, not +x).
    let shape = from_text_brep(&fixture("mirrored_box.brep")).unwrap();
    assert!(summarize(&shape).unwrap().valid);
    let d = described(&shape);
    let inside = |p: [f64; 3]| {
        (-10.0..=0.0).contains(&p[0])
            && (0.0..=20.0).contains(&p[1])
            && (0.0..=30.0).contains(&p[2])
    };
    let freecad = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
    ];
    assert_eq!(d.faces.len(), 6);
    for (i, (face, want)) in d.faces.iter().zip(freecad).enumerate() {
        let Surface::Plane { normal } = face.surface else {
            panic!("Face{} isn't a plane", i + 1)
        };
        let at = |t: f64| [0, 1, 2].map(|k| face.centroid[k] + t * normal[k]);
        assert!(
            !inside(at(0.01)) && inside(at(-0.01)),
            "Face{}'s normal {normal:?} faces into the box (FreeCAD's is {want:?})",
            i + 1
        );
        assert_point(&format!("Face{}'s normal", i + 1), normal, want, 1e-12);
    }
}

#[test]
fn a_mirrored_boxs_planes_measure_its_volume_by_the_divergence_theorem() {
    // The same mirrored box: for a solid of flat faces, V = ⅓ Σ A·(n · c) over its faces
    // (n outward, c the centre of area), 6000.
    //
    // Broke before #121's fix: with every normal pointing in, the sum is −6000.
    let shape = from_text_brep(&fixture("mirrored_box.brep")).unwrap();
    let (_, volume, _) = flat_sums(&described(&shape));
    assert_close("⅓ Σ A (n · c)", volume, 6000.0, 1e-9);
}

// ---- Chamfers ----

#[test]
fn every_chamfer_face_of_a_box_faces_out_of_it() {
    // A 10 × 20 × 30 box with each of its twelve edges chamfered 1 mm in turn: the chamfer
    // face bisects the two faces it joins, so its outward normal is the sum of their
    // outward normals over √2, pointing away from the box's middle (5, 10, 15); and the
    // solid's flat faces satisfy Σ A·n = 0. FreeCAD 1.1.3 agrees: on a 1 × 1 × 10.15 box
    // with Edge3 chamfered 0.05, the chamfer face (Face3, Forward) has `normalAt`
    // (−0.7071, 0.7071, 0), out of the box, while its plane's axis is (0.7071, −0.7071, 0)
    // (left-handed: `Surface.normal`, Du × Dv, is the opposite).
    //
    // Broke before #121's fix: OCCT's chamfer (ChFiKPart) makes the chamfer face's plane
    // left-handed, and
    // describe takes the plane's axis for its normal: a chamfer face is said to face into
    // the solid, on a solid Silico's own Chamfer builds.
    let shape = make(&Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
    .unwrap();
    let middle = [5.0, 10.0, 15.0];
    let mut wrong = Vec::new();
    for edge in 1..=12 {
        let built = chamfer(&shape, &[edge], ChamferSize::Equal { distance: 1.0 }, false).unwrap();
        let d = described(&built.shape);
        let slanted: Vec<_> = d
            .faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Plane { normal } if normal.iter().filter(|c| c.abs() > 0.5).count() != 1))
            .collect();
        assert_eq!(slanted.len(), 1, "Edge{edge}: {:?}", d.faces);
        let Surface::Plane { normal } = slanted[0].surface else {
            unreachable!()
        };
        let away = [0, 1, 2].map(|k| slanted[0].centroid[k] - middle[k]);
        let (closure, volume, _) = flat_sums(&d);
        if dot(normal, away) <= 0.0 || closure > 1e-9 {
            wrong.push(format!(
                "Edge{edge}: normal {normal:?} at {:?}, Σ A·n {closure:e}, ⅓ Σ A (n · c) {volume}",
                slanted[0].centroid
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "chamfer faces facing in:\n{}",
        wrong.join("\n")
    );
}

// ---- INTERNAL and EXTERNAL faces ----

#[test]
fn a_box_with_an_internal_or_external_face_is_described_every_edge_with_its_ends() {
    // A 10 × 20 × 30 box whose face at x = 0 is INTERNAL, or EXTERNAL, in its shell
    // (tests/fixtures/brep/README.md). Silico reads, measures, and meshes it; FreeCAD lists
    // each of its edges' two vertices (`Edge.Vertexes`). Each of its twelve edges is a
    // line between two of its corners, as long as the distance between them.
    //
    // Broke before #121's fix: TopExp::MapShapes keeps each edge as first met, so the four edges of
    // the
    // INTERNAL (or EXTERNAL) Face1 are kept INTERNAL (EXTERNAL), and TopExp::Vertices with
    // the orientation taken gives neither of their vertices (an INTERNAL vertex is neither
    // FORWARD nor REVERSED): describe throws "an edge without both its vertices" and the
    // whole shape is refused, as a kernel failure that names no edge.
    for name in ["box_internal_face1.bin", "box_external_face1.bin"] {
        let shape = from_brep(&with_crc(fixture(name))).unwrap();
        let d = describe_whole(&shape)
            .unwrap_or_else(|e| panic!("{name}: refused: {e} ({:?})", e.kind()));
        d.validate(1).unwrap();
        assert_eq!(d.edges.len(), 12, "{name}");
        for (i, e) in d.edges.iter().enumerate() {
            let span = norm([0, 1, 2].map(|k| e.end[k] - e.start[k]));
            assert_close(&format!("{name}: Edge{}", i + 1), e.length, span, 1e-12);
        }
    }
}

// ---- Ellipses, arcs, and their measures ----

/// A slab above the plane through (0, 0, 10) that rises `degrees` along +x: a 40 × 40
/// rectangle on it, extruded 30 along its upward normal.
fn slab(degrees: f64) -> Shape {
    let t = libm::tan(degrees.to_radians());
    let at = |x: f64, y: f64| [x, y, 10.0 + x * t];
    let profile = Profile {
        points: vec![
            at(-20.0, -20.0),
            at(20.0, -20.0),
            at(20.0, 20.0),
            at(-20.0, 20.0),
        ],
    };
    extrude(&profile, 30.0, None).unwrap().shape
}

/// An ellipse's perimeter, by the trapezoid rule over its whole period (exponentially
/// convergent for a smooth periodic integrand), independent of OCCT.
fn ellipse_perimeter(a: f64, b: f64) -> f64 {
    let n = 4096;
    let h = 2.0 * PI / n as f64;
    (0..n)
        .map(|i| {
            let t = i as f64 * h;
            let (s, c) = (libm::sin(t), libm::cos(t));
            libm::sqrt(a * a * s * s + b * b * c * c)
        })
        .sum::<f64>()
        * h
}

#[test]
fn a_cylinder_cut_on_a_slant_has_an_ellipse_of_its_measures_and_a_top_facing_up_the_slant() {
    // A cylinder r 5, h 20, less everything above the plane z = 10 + x · tan 30°: its rim
    // there is an ellipse of semi-axes 5 / cos 30° and 5, perimeter by the trapezoid rule;
    // its top, that ellipse's inside, of area π · a · b, its centre of area (0, 0, 10), its
    // outward normal (−sin 30°, 0, cos 30°).
    let cylinder = make(&Primitive::Cylinder {
        radius: 5.0,
        height: 20.0,
    })
    .unwrap();
    let cut = boolean(BooleanOp::Cut, &cylinder, &slab(30.0))
        .unwrap()
        .shape;
    let d = described(&cut);
    let theta = 30f64.to_radians();
    let (a, b) = (5.0 / libm::cos(theta), 5.0);
    let ellipses: Vec<_> = d
        .edges
        .iter()
        .filter(|e| matches!(e.curve, EdgeCurve::Ellipse { .. }))
        .collect();
    assert_eq!(ellipses.len(), 1, "{:?}", d.edges);
    let EdgeCurve::Ellipse {
        axis,
        major_radius,
        minor_radius,
    } = &ellipses[0].curve
    else {
        unreachable!()
    };
    assert_close("the major radius", *major_radius, a, 1e-12);
    assert_close("the minor radius", *minor_radius, b, 1e-12);
    assert_point("the ellipse's centre", axis.origin, [0.0, 0.0, 10.0], 1e-9);
    assert_close(
        "the ellipse's perimeter",
        ellipses[0].length,
        ellipse_perimeter(a, b),
        1e-9,
    );
    let top = d
        .faces
        .iter()
        .find(|f| matches!(f.surface, Surface::Plane { normal } if normal[0] != 0.0))
        .unwrap();
    let Surface::Plane { normal } = top.surface else {
        unreachable!()
    };
    assert_point(
        "the top's normal",
        normal,
        [-libm::sin(theta), 0.0, libm::cos(theta)],
        1e-12,
    );
    assert_close("the top's area", top.area, PI * a * b, 1e-9);
    assert_point("the top's centre", top.centroid, [0.0, 0.0, 10.0], 1e-9);
}

#[test]
fn a_turned_rectangles_arcs_measure_their_turn_and_its_flat_faces_face_out() {
    // The rectangle x ∈ [2, 5], z ∈ [0, 3] in the XZ plane turned about the z axis by 90°,
    // 180°, and 360°. Its arcs, of radius 2 and 5, are each r · angle long; for a part
    // turn, each arc's middle is halfway round, at angle/2 from +x. Its flat faces: the
    // bottom (z = 0) faces −z, the top (z = 3) +z, as the solid lies in z ∈ [0, 3]; for a
    // part turn, its end at the start faces −y, and at 90° the other end faces −x
    // (FreeCAD 1.1.3's `Face.normalAt` on `Face.revolve` of the same face: Face1 −z,
    // Face3 +z, Face5 −y, Face6 −x). Volume by Pappus: area 9 times the centroid's path,
    // 3.5 · angle.
    //
    // Broke before #121's fix: OCCT's revolve (BRepSweep_Rotation) gives the bottom a plane whose
    // axes are
    // left-handed: its axis is +z, its normal (Du × Dv) −z, and the face is FORWARD.
    // describe takes the axis for the normal, so the bottom is said to face +z, into the
    // solid (FreeCAD's Face1 normalAt is −z), on a solid Silico's own Revolve builds.
    let profile = Profile {
        points: vec![
            [2.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [5.0, 0.0, 3.0],
            [2.0, 0.0, 3.0],
        ],
    };
    let axis = Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
    };
    for degrees in [90.0, 180.0, 360.0] {
        let angle = f64::to_radians(degrees);
        let built = revolve(&profile, &axis, degrees).unwrap();
        let d = described(&built.shape);
        assert_close(
            &format!("{degrees}°: the volume"),
            summarize(&built.shape).unwrap().volume,
            9.0 * 3.5 * angle,
            1e-9,
        );
        let mut arcs = 0;
        for e in &d.edges {
            if let EdgeCurve::Circle { axis, radius } = &e.curve {
                arcs += 1;
                assert!(*radius == 2.0 || *radius == 5.0, "{radius}");
                assert_close(
                    &format!("{degrees}°: an arc's length"),
                    e.length,
                    radius * angle,
                    1e-12,
                );
                assert_eq!(axis.direction[2].abs(), 1.0);
                if degrees < 360.0 {
                    let half = angle / 2.0;
                    assert_point(
                        &format!("{degrees}°: an arc's middle"),
                        [e.middle[0], e.middle[1].abs(), 0.0],
                        [radius * libm::cos(half), radius * libm::sin(half), 0.0],
                        1e-9,
                    );
                }
            }
        }
        // Four: the top's and the bottom's, at radius 2 and 5 (for a whole turn, the tube's
        // annular top and bottom are each bounded by two whole circles).
        assert_eq!(arcs, 4, "{degrees}°");
        for (i, f) in d.faces.iter().enumerate() {
            let Surface::Plane { normal } = f.surface else {
                continue;
            };
            let c = f.centroid;
            let want = if c[2] == 0.0 {
                [0.0, 0.0, -1.0]
            } else if c[2] == 3.0 {
                [0.0, 0.0, 1.0]
            } else if c[1].abs() < 1e-12 && c[0] > 0.0 {
                [0.0, -1.0, 0.0]
            } else if c[0].abs() < 1e-12 {
                [-1.0, 0.0, 0.0]
            } else {
                // 180°: the far end, at y = 0 on the −x side, faces −y too.
                [0.0, -1.0, 0.0]
            };
            assert_point(
                &format!("{degrees}°: Face{}'s normal (centre {c:?})", i + 1),
                normal,
                want,
                1e-12,
            );
        }
    }
}

#[test]
fn a_cut_that_leaves_an_internal_edge_is_described_with_that_edge_and_its_ends() {
    // A triangular prism (the triangle of radius 1 about (5, 0, 0) on the plane of normal
    // (sin 2.1511 · cos π, sin 2.1511 · sin π, cos 2.1511), as `polygon` lays it out, 0.5
    // along that normal) less a 10 mm cube from (5, 0, 0): the cube's edge x = 5, z = 0
    // lies in the prism's bottom face, so OCCT's cut leaves it there as an INTERNAL edge.
    // FreeCAD 1.1.3 makes the same: a valid compound of 5 faces, 11 edges, 8 vertices,
    // volume 0.6495, its Edge11 INTERNAL, from (5, 0, 0) to (5, 0.5, 0), 0.5 long, both
    // its vertices listed. Silico's boolean accepts it as valid.
    //
    // Broke before #121's fix: describe refuses the whole solid, "Standard_Failure: an edge without
    // both its
    // vertices" (TopExp::Vertices with the orientation taken gives an INTERNAL edge no
    // FORWARD or REVERSED vertex), naming no edge: the result of two of Silico's own
    // operations can't be listed.
    let n = direction(2.1511169518519266, PI);
    let prism = extrude(&polygon([5.0, 0.0, 0.0], n, 1.0, 3, 0.0), 0.5, None)
        .unwrap()
        .shape;
    let cube = translate(
        &make(&Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: 10.0,
        })
        .unwrap(),
        [5.0, 0.0, 0.0],
    )
    .unwrap()
    .shape;
    let cut = boolean(BooleanOp::Cut, &prism, &cube).unwrap().shape;
    let summary = summarize(&cut).unwrap();
    assert!(summary.valid);
    assert_close("the volume", summary.volume, 0.649519052838329, 1e-9);
    let d = describe_whole(&cut).unwrap_or_else(|e| panic!("refused: {e} ({:?})", e.kind()));
    d.validate(1).unwrap();
    assert_eq!((d.faces.len(), d.edges.len(), d.vertices.len()), (5, 11, 8));
    let internal: Vec<_> = d
        .edges
        .iter()
        .filter(|e| (e.length - 0.5).abs() < 1e-9 && e.start[0] == 5.0 && e.end[0] == 5.0)
        .collect();
    assert_eq!(internal.len(), 1, "{:?}", d.edges);
}

// ---- Cones and tori at their limits ----

#[test]
fn a_cone_from_a_point_has_its_apex_at_its_base() {
    // r 0 to 4, h 6: its apex at the origin, its half-angle atan(4/6); its radius
    // at its axis's origin at least 0, and origin − direction · radius / tan(half-angle)
    // the apex; its side's area π · 4 · √(4² + 6²).
    let d = described(
        &make(&Primitive::Cone {
            radius1: 0.0,
            radius2: 4.0,
            height: 6.0,
        })
        .unwrap(),
    );
    let side = d
        .faces
        .iter()
        .find(|f| matches!(f.surface, Surface::Cone { .. }))
        .unwrap();
    let Surface::Cone {
        axis,
        radius,
        half_angle,
    } = &side.surface
    else {
        unreachable!()
    };
    // +0.5880026035475675, widening up its axis, as FreeCAD 1.1.3's Part::Cone gives it.
    assert_close("the half-angle", *half_angle, libm::atan(4.0 / 6.0), 1e-12);
    let apex =
        [0, 1, 2].map(|k| axis.origin[k] - axis.direction[k] * radius / libm::tan(*half_angle));
    assert_point("the apex", apex, [0.0; 3], 1e-9);
    assert_close(
        "the side's area",
        side.area,
        PI * 4.0 * libm::sqrt(52.0),
        1e-12,
    );
    let apexes: Vec<_> = d
        .edges
        .iter()
        .filter(|e| e.curve == EdgeCurve::Degenerated)
        .collect();
    assert_eq!(apexes.len(), 1);
    assert_point("the degenerated edge", apexes[0].start, [0.0; 3], 1e-12);
}

#[test]
fn a_cone_whose_radii_differ_by_the_least_still_has_a_finite_apex_or_is_refused() {
    // r 4 to 4 − 2e-7 over 6: a half-angle of about 3.3e-8, its apex 1.2e8 away. Either
    // OCCT makes it and every value is finite and checked, its side's area
    // π (r1 + r2) √((r1 − r2)² + h²), or it's refused with a reason.
    let (r1, r2, h) = (4.0, 4.0 - 2e-7, 6.0);
    let Ok(shape) = make(&Primitive::Cone {
        radius1: r1,
        radius2: r2,
        height: h,
    }) else {
        return;
    };
    let d = described(&shape);
    let side = d
        .faces
        .iter()
        .find(|f| !matches!(f.surface, Surface::Plane { .. }))
        .unwrap();
    let want = PI * (r1 + r2) * libm::sqrt((r1 - r2) * (r1 - r2) + h * h);
    assert_close("the side's area", side.area, want, 1e-9);
    if let Surface::Cone {
        radius, half_angle, ..
    } = side.surface
    {
        assert!((radius / libm::tan(half_angle)).is_finite());
    }
}

#[test]
fn a_cone_of_equal_radii_is_a_cylinder_or_is_refused() {
    // r 4 to 4: no apex at all. If OCCT makes it, its side isn't a cone of half-angle 0
    // (whose apex would be infinitely far): it's a cylinder of radius 4, area 2π · 4 · 6.
    let Ok(shape) = make(&Primitive::Cone {
        radius1: 4.0,
        radius2: 4.0,
        height: 6.0,
    }) else {
        return;
    };
    let d = described(&shape);
    for f in &d.faces {
        if let Surface::Cone { half_angle, .. } = f.surface {
            assert!(half_angle != 0.0, "a cone of half-angle 0: {f:?}");
        }
    }
    let side = d
        .faces
        .iter()
        .find(|f| !matches!(f.surface, Surface::Plane { .. }))
        .unwrap();
    assert_close("the side's area", side.area, 48.0 * PI, 1e-9);
}

#[test]
fn a_horn_torus_and_one_just_past_it_have_their_radii_and_area() {
    // R = r = 2 (the tube touches the axis), and r = 2 + 5e-8, which the primitive's check
    // allows (within the length tolerance): radii as given, area 4π²Rr, every value
    // checked.
    for minor in [2.0, 2.0 + 5e-8] {
        let Ok(shape) = make(&Primitive::Torus {
            major_radius: 2.0,
            minor_radius: minor,
        }) else {
            continue;
        };
        let d = described(&shape);
        assert_eq!(d.faces.len(), 1);
        // OCCT makes a horn torus's face a surface of revolution, not a torus, when it's
        // revolved as Silico and FreeCAD 1.1.3's Part::Torus feature build it: describe says
        // what OCCT made. (`Part.makeTorus(2, 2)`, through BRepPrimAPI_MakeTorus, instead
        // gives a toroid.)
        match d.faces[0].surface {
            Surface::Torus {
                major_radius,
                minor_radius,
                ..
            } => assert_eq!((major_radius, minor_radius), (2.0, minor)),
            Surface::Revolution => {}
            ref other => panic!("{other:?}"),
        }
        assert_close(
            "the area",
            d.faces[0].area,
            4.0 * PI * PI * 2.0 * minor,
            1e-9,
        );
    }
}

// ---- The ends of the length range ----

#[test]
fn boxes_and_cylinders_at_the_ends_of_the_length_range_measure_exactly() {
    // The smallest and largest lengths a primitive takes (just above 1e-7, and 1e6): a
    // box's areas and lengths, and a cylinder's rim, 2πr, to the relative precision.
    for s in [1.0000001e-7, 1e6 / 3.0] {
        let d = described(
            &make(&Primitive::Box {
                dx: s,
                dy: 2.0 * s,
                dz: 3.0 * s,
            })
            .unwrap(),
        );
        let mut areas: Vec<f64> = d.faces.iter().map(|f| f.area).collect();
        areas.sort_by(f64::total_cmp);
        for (got, want) in areas.iter().zip([2.0, 2.0, 3.0, 3.0, 6.0, 6.0]) {
            assert_close(&format!("{s}: an area"), *got, want * s * s, 1e-9);
        }
        for e in &d.edges {
            let span = norm([0, 1, 2].map(|k| e.end[k] - e.start[k]));
            assert_close(&format!("{s}: an edge"), e.length, span, 1e-9);
        }
        let d = described(
            &make(&Primitive::Cylinder {
                radius: s,
                height: s,
            })
            .unwrap(),
        );
        for e in &d.edges {
            if let EdgeCurve::Circle { radius, .. } = e.curve {
                assert_close(&format!("{s}: a rim"), e.length, 2.0 * PI * radius, 1e-9);
                // A whole circle's middle is opposite its start, 2r away.
                assert_close(
                    &format!("{s}: a rim's middle"),
                    norm([0, 1, 2].map(|k| e.middle[k] - e.start[k])),
                    2.0 * radius,
                    1e-9,
                );
            }
        }
    }
}

// ---- Garbage to a worker ----

#[test]
fn a_worker_asked_to_describe_garbage_says_so_and_goes_on() {
    // Bytes that aren't a B-rep, none at all, and a B-rep cut short: each a structured
    // refusal, and the worker then describes a box as it should.
    let mut worker = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let good = worker
        .make(Primitive::Box {
            dx: 1.0,
            dy: 2.0,
            dz: 3.0,
        })
        .unwrap();
    let mut cut_short = good.brep().as_bytes().to_vec();
    cut_short.truncate(cut_short.len() / 2);
    for (what, bytes) in [
        ("garbage", b"not a brep at all".to_vec()),
        ("nothing", Vec::new()),
        ("cut short", cut_short.clone()),
        (
            "cut short, its checksum fixed",
            with_crc(cut_short[..cut_short.len() - 4].to_vec()),
        ),
    ] {
        let err = worker
            .describe(&ShapeData::unchecked(Blob::inline(bytes)))
            .unwrap_err();
        assert!(
            matches!(
                err.code(),
                ErrorCode::InvalidInput | ErrorCode::KernelFailed
            ),
            "{what}: {err} ({:?})",
            err.code()
        );
        let d = worker.describe(&good).unwrap();
        assert_eq!((d.faces.len(), d.edges.len(), d.vertices.len()), (6, 12, 8));
    }
}

// ---- Random flat solids: the divergence theorem ----

/// For a solid of flat faces: Σ A·n over its faces (0 for a closed solid), as its length
/// relative to the total area, and ⅓ Σ A·(n · c), its volume.
fn flat_sums(d: &Description) -> (f64, f64, f64) {
    let (mut sum, mut total, mut volume, mut scale) = ([0.0; 3], 0.0, 0.0, 0.0);
    for f in &d.faces {
        let Surface::Plane { normal } = f.surface else {
            panic!("a face that isn't flat: {f:?}")
        };
        for k in 0..3 {
            sum[k] += f.area * normal[k];
        }
        total += f.area;
        volume += f.area * dot(normal, f.centroid) / 3.0;
        scale += f.area * dot(normal, f.centroid).abs() / 3.0;
    }
    (norm(sum) / total, volume, scale)
}

/// A unit vector from two angles.
fn direction(polar: f64, azimuth: f64) -> [f64; 3] {
    let (sp, cp) = (libm::sin(polar), libm::cos(polar));
    [sp * libm::cos(azimuth), sp * libm::sin(azimuth), cp]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A convex polygon of `n` corners on a circle of radius `r` about `origin`, in the plane
/// of normal `n̂`, counterclockwise seen from its tip.
fn polygon(origin: [f64; 3], normal: [f64; 3], r: f64, corners: usize, turn: f64) -> Profile {
    let helper = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = cross(normal, helper);
    let u = u.map(|x| x / norm(u));
    let v = cross(normal, u);
    let points = (0..corners)
        .map(|i| {
            let a = turn + 2.0 * PI * i as f64 / corners as f64;
            let (c, s) = (r * libm::cos(a), r * libm::sin(a));
            [0, 1, 2].map(|k| origin[k] + c * u[k] + s * v[k])
        })
        .collect();
    Profile { points }
}

#[derive(Debug, Clone)]
enum Part {
    Box([f64; 3], [f64; 3]),
    Prism {
        origin: [f64; 3],
        normal: (f64, f64),
        radius: f64,
        corners: usize,
        turn: f64,
        length: f64,
        lean: Option<(f64, f64)>,
    },
    /// A box at a corner, its edges `edges` chamfered by `fraction` of its least side.
    Chamfered([f64; 3], [f64; 3], Vec<u32>, f64),
}

fn part() -> impl Strategy<Value = Part> {
    let coordinate = prop_oneof![Just(0.0), Just(5.0), -10.0..10.0f64];
    let size = prop_oneof![Just(10.0), Just(1e-3), 0.5..15.0f64];
    let angle = prop_oneof![Just(0.0), Just(FRAC_PI_2), Just(PI), 0.0..PI];
    prop_oneof![
        (
            [coordinate.clone(), coordinate.clone(), coordinate.clone()],
            [size.clone(), size.clone(), size]
        )
            .prop_map(|(at, size)| Part::Box(at, size)),
        (
            [coordinate.clone(), coordinate.clone(), coordinate],
            (angle.clone(), angle.clone()),
            1.0..8.0f64,
            3usize..9,
            0.0..1.0f64,
            0.5..12.0f64,
            proptest::option::of((0.0..0.45f64, angle)),
        )
            .prop_map(|(origin, normal, radius, corners, turn, length, lean)| {
                Part::Prism {
                    origin,
                    normal,
                    radius,
                    corners,
                    turn,
                    length,
                    lean,
                }
            }),
        (
            [-5.0..5.0f64, -5.0..5.0f64, -5.0..5.0f64],
            [1.0..12.0f64, 1.0..12.0f64, 1.0..12.0f64],
            proptest::collection::btree_set(1u32..=12, 1..4),
            0.05..0.45f64,
        )
            .prop_map(|(at, size, edges, fraction)| Part::Chamfered(
                at,
                size,
                edges.into_iter().collect(),
                fraction
            )),
    ]
}

fn build(part: &Part) -> Option<Shape> {
    match *part {
        Part::Box(at, [dx, dy, dz]) => {
            let b = make(&Primitive::Box { dx, dy, dz }).ok()?;
            Some(translate(&b, at).ok()?.shape)
        }
        Part::Chamfered(at, [dx, dy, dz], ref edges, fraction) => {
            let b = make(&Primitive::Box { dx, dy, dz }).ok()?;
            let distance = fraction * dx.min(dy).min(dz);
            let c = chamfer(&b, edges, ChamferSize::Equal { distance }, false).ok()?;
            Some(translate(&c.shape, at).ok()?.shape)
        }
        Part::Prism {
            origin,
            normal,
            radius,
            corners,
            turn,
            length,
            lean,
        } => {
            let n = direction(normal.0, normal.1);
            let profile = polygon(origin, n, radius, corners, turn);
            // A lean off the normal, by a polar angle under 0.45 rad about it.
            let direction = lean.map(|(tilt, about)| {
                let helper = if n[0].abs() < 0.9 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                };
                let u = cross(n, helper);
                let u = u.map(|x| x / norm(u));
                let v = cross(n, u);
                let (s, c) = (libm::sin(tilt), libm::cos(tilt));
                let (sa, ca) = (libm::sin(about), libm::cos(about));
                [0, 1, 2].map(|k| c * n[k] + s * (ca * u[k] + sa * v[k]))
            });
            Some(extrude(&profile, length, direction).ok()?.shape)
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        failure_persistence: None,
        // Fixed, so a red run is red every time (as adversarial_fillet's).
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xde5c_01be),
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_flat_solid_is_closed_and_measures_its_volume_by_its_faces(
        a in part(),
        b in part(),
        op in prop_oneof![Just(None), Just(Some(BooleanOp::Cut)), Just(Some(BooleanOp::Fuse)), Just(Some(BooleanOp::Common))],
    ) {
        // Boxes, chamfered boxes, and prisms of convex polygons on any plane, straight or
        // leaning, alone or cut, fused, or in common: every face flat, so Σ A·n = 0 and
        // ⅓ Σ A·(n · c) is the volume OCCT measures. A normal facing in breaks both.
        //
        // BREAKS, two ways: a chamfer face's plane is left-handed, and describe says it
        // faces in (minimized: every_chamfer_face_of_a_box_faces_out_of_it; a box less a
        // 1 × 1 × 10.15 box with Edge3 chamfered 0.05 gives Σ A·n of 2.4e-3 of the area);
        // and some cuts leave an INTERNAL edge where one solid's edge lies in the other's
        // face, and describe refuses the solid, "an edge without both its vertices"
        // (a_cut_that_leaves_an_internal_edge_is_described_with_that_edge_and_its_ends).
        // Without chamfers and with those refusals skipped, the 1000 cases hold.
        let Some(sa) = build(&a) else { return Ok(()) };
        let shape = match op {
            None => sa,
            Some(op) => {
                let Some(sb) = build(&b) else { return Ok(()) };
                match boolean(op, &sa, &sb) {
                    Ok(built) => built.shape,
                    Err(_) => return Ok(()),
                }
            }
        };
        let summary = summarize(&shape).unwrap();
        if !summary.valid || summary.volume <= 1e-9 {
            return Ok(());
        }
        let d = describe_whole(&shape).unwrap();
        prop_assert_eq!(d.validate(1), Ok(()));
        prop_assert_eq!(&describe_whole(&shape).unwrap(), &d, "describe twice, differently");
        let (closure, volume, scale) = flat_sums(&d);
        prop_assert!(closure < 1e-8, "Σ A·n is {closure:e} of the area: {d:?}");
        // To 1e-8 of the terms' size (⅓ Σ A |n · c|), not of the volume: a sliver far from
        // the origin is a difference of terms each ~1e9 times it (#121's review: 0.001 ×
        // 0.5 × 0.001 at (5, −8, 5), 5.157e-9, off by 3.4e-16).
        prop_assert!(
            (volume - summary.volume).abs() <= 1e-8 * scale,
            "⅓ Σ A (n · c) = {volume}, volume {}, terms {scale}", summary.volume
        );
        for (i, e) in d.edges.iter().enumerate() {
            prop_assert!(e.curve.is_straight(), "Edge{} isn't straight: {e:?}", i + 1);
            let span = norm([0, 1, 2].map(|k| e.end[k] - e.start[k]));
            prop_assert!(relative_error(e.length, span) < 1e-9, "Edge{}: {e:?}", i + 1);
            let mid = [0, 1, 2].map(|k| (e.start[k] + e.end[k]) / 2.0);
            prop_assert!(norm([0, 1, 2].map(|k| e.middle[k] - mid[k])) <= 1e-9 * (1.0 + span));
        }
    }
}

#[test]
fn bsplines_beziers_extrusions_and_revolutions_are_each_their_kind_with_their_poles() {
    // curves_and_surfaces.brep (the fixtures' README), each element as FreeCAD 1.1.3 types
    // it (`type(Face.Surface)`, `type(Edge.Curve)`, its `Degree` and `NbPoles`, its
    // `Length`), in order; straight where it's a line or has two poles (FreeCAD's
    // de14265ba7).
    let shape = from_text_brep(&fixture("curves_and_surfaces.brep")).unwrap();
    let d = described(&shape);
    let faces: Vec<&Surface> = d.faces.iter().map(|f| &f.surface).collect();
    // The Bézier square's and the revolved Bézier's areas, as FreeCAD's Face.Area (OCCT's
    // default integration) and a composite Gauss integration of |Su × Sv| (#121's review)
    // agree.
    for (face, area) in [(1, 1.0), (3, 8.35476640348216)] {
        assert_close("a face's area", d.faces[face].area, area, 1e-9);
    }
    // The ruled surface is the cubic B-spline (3.2682882396736437 long) extruded 2 at right
    // angles to its plane: 2 × that = 6.536576479347287 by hand. OCCT gives
    // 6.53657440325774 either way it integrates, 3.2e-7 short: its wrong answer, as the
    // extruded face's below. Right, or that wrong answer.
    let ruled = d.faces[0].area;
    assert!(
        (ruled - 2.0 * 3.2682882396736437).abs() < 1e-9 * ruled
            || (ruled - 6.53657440325774).abs() < 1e-12,
        "{ruled}"
    );
    // The extruded B-spline's is, by hand, its curve's length times the 2 it's extruded:
    // 2 × 4.40405587355 = 8.8081117471. OCCT's precise integration (to 1e-9, saying it got
    // to 2e-16) gives 8.83202504977072, 0.27% over, and its default one (FreeCAD's) gives
    // 8.808420588300923: OCCT's wrong answer, accepted as FreeCAD accepts OCCT's
    // (docs/design/m4.4-describe.md, Known wrong results). Right, or that wrong answer.
    let extruded = d.faces[2].area;
    assert!(
        (extruded - 2.0 * 4.40405587355).abs() < 1e-9 * extruded
            || (extruded - 8.83202504977072).abs() < 1e-12,
        "{extruded}"
    );
    assert_eq!(
        faces,
        [
            &Surface::Bspline,
            &Surface::Bezier,
            &Surface::Extrusion,
            &Surface::Revolution
        ]
    );
    let bezier = |degree, poles| EdgeCurve::Bezier { degree, poles };
    let bspline = |degree, poles| EdgeCurve::Bspline { degree, poles };
    let want = [
        (bezier(1, 2), 5.0),
        (bezier(2, 3), 2.29558714939),
        (bspline(1, 2), 4.0),
        (bspline(3, 5), 4.40405587355),
        (bspline(3, 4), 3.26828823967),
        (bspline(1, 2), 2.0),
        (bspline(3, 4), 3.26828823967),
        (bspline(1, 2), 2.0),
        (bezier(1, 2), 1.0),
        (bezier(1, 2), 1.0),
        (bezier(1, 2), 1.0),
        (bezier(1, 2), 1.0),
        (EdgeCurve::Line, 2.0),
        (EdgeCurve::Line, 2.0),
        (bspline(3, 5), 4.40405587355),
        (bspline(3, 5), 4.40405587355),
    ];
    for (i, (e, (curve, length))) in d.edges.iter().zip(&want).enumerate() {
        let what = format!("Edge{}", i + 1);
        assert_eq!(&e.curve, curve, "{what}");
        assert_close(&what, e.length, *length, 1e-11);
        let straight = matches!(
            curve,
            EdgeCurve::Line
                | EdgeCurve::Bezier { poles: 2, .. }
                | EdgeCurve::Bspline { poles: 2, .. }
        );
        assert_eq!(e.curve.is_straight(), straight, "{what}");
    }
    // Then the turn's two arcs, of radius 2 (the Bézier's ends are 2 from the x axis), a
    // quarter turn, so π long; and the turned Bézier at each end.
    assert_eq!(d.edges.len(), 20);
    // FreeCAD's Edge17 and Edge18: circles of radius 2 about (0, 20, 0) and (2, 20, 0).
    for (e, centre) in d.edges[16..18]
        .iter()
        .zip([[0.0, 20.0, 0.0], [2.0, 20.0, 0.0]])
    {
        let EdgeCurve::Circle { axis, radius } = &e.curve else {
            panic!("{e:?}")
        };
        assert_close("a turned end's radius", *radius, 2.0, 1e-12);
        assert_point("a turned end's centre", axis.origin, centre, 1e-12);
        assert_close("a turned end's arc", e.length, std::f64::consts::PI, 1e-11);
    }
    for e in &d.edges[18..] {
        assert_eq!(e.curve, bezier(2, 3));
        assert_close("a turned Bézier", e.length, 2.29558714939, 1e-11);
    }
}
