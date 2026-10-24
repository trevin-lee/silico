//! M4.4: `describe`, each element's geometry and measures (docs/design/m4.4-describe.md),
//! against closed forms: a box's faces, edges, and corners; a cylinder's side, caps, rims,
//! and seam; a sphere's and a cone's degenerated edges; a torus's radii; and the normals of
//! faces a cut reverses, which point out of the solid.

use super::close;

use std::f64::consts::PI;

use close::assert_close;
use silico_kernel::{BooleanOp, Primitive, boolean, describe, make, translate};
use silico_protocol::{Description, EdgeCurve, Surface};

fn described(p: Primitive) -> Description {
    let d = describe(&make(&p).unwrap()).unwrap();
    d.check().unwrap();
    d
}

fn assert_point(what: &str, got: [f64; 3], want: [f64; 3]) {
    let off = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0.0, f64::max);
    assert!(off < 1e-9, "{what}: got {got:?}, want {want:?}");
}

#[test]
fn a_boxs_faces_are_planes_facing_out_from_their_middles_with_their_areas() {
    // A 10 × 20 × 30 box, its faces in OCCT's order (FreeCAD's Face1 to Face6): x = 0 and
    // x = 10 (20 × 30 = 600 each), y = 0 and y = 20 (10 × 30 = 300), z = 0 and z = 30
    // (10 × 20 = 200), each facing out of the box, each centre of area its face's middle.
    let d = described(Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    });
    let want = [
        ([-1.0, 0.0, 0.0], 600.0, [0.0, 10.0, 15.0]),
        ([1.0, 0.0, 0.0], 600.0, [10.0, 10.0, 15.0]),
        ([0.0, -1.0, 0.0], 300.0, [5.0, 0.0, 15.0]),
        ([0.0, 1.0, 0.0], 300.0, [5.0, 20.0, 15.0]),
        ([0.0, 0.0, -1.0], 200.0, [5.0, 10.0, 0.0]),
        ([0.0, 0.0, 1.0], 200.0, [5.0, 10.0, 30.0]),
    ];
    assert_eq!(d.faces.len(), 6);
    for (i, (face, (normal, area, centroid))) in d.faces.iter().zip(want).enumerate() {
        let what = format!("Face{}", i + 1);
        assert_eq!(face.surface, Surface::Plane { normal }, "{what}");
        assert_close(&what, face.area, area, 1e-12);
        assert_point(&what, face.centroid, centroid);
    }
}

#[test]
fn a_boxs_edges_are_lines_of_its_sides_and_its_vertices_its_corners() {
    // Twelve lines, four each of 10, 20, and 30, each from one corner to another, its
    // middle halfway; eight vertices, one at each corner.
    let d = described(Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    });
    assert_eq!(d.edges.len(), 12);
    let mut lengths: Vec<f64> = Vec::new();
    for (i, e) in d.edges.iter().enumerate() {
        let what = format!("Edge{}", i + 1);
        assert_eq!(e.curve, EdgeCurve::Line, "{what}");
        assert!(e.curve.is_straight());
        assert!(!e.degenerated);
        let span: f64 = (0..3).map(|k| (e.end[k] - e.start[k]).powi(2)).sum();
        assert_close(&what, e.length, span.sqrt(), 1e-12);
        assert_point(
            &what,
            e.middle,
            [0, 1, 2].map(|k| (e.start[k] + e.end[k]) / 2.0),
        );
        lengths.push(e.length);
    }
    lengths.sort_by(f64::total_cmp);
    let want: Vec<f64> = [10.0, 20.0, 30.0].iter().flat_map(|&l| [l; 4]).collect();
    for (got, want) in lengths.iter().zip(&want) {
        assert_close("an edge's length", *got, *want, 1e-12);
    }
    let mut corners = d.vertices.clone();
    corners.sort_by(|a, b| {
        a.iter()
            .zip(b)
            .map(|(x, y)| x.total_cmp(y))
            .fold(std::cmp::Ordering::Equal, std::cmp::Ordering::then)
    });
    let mut want = Vec::new();
    for x in [0.0, 10.0] {
        for y in [0.0, 20.0] {
            for z in [0.0, 30.0] {
                want.push([x, y, z]);
            }
        }
    }
    assert_eq!(corners, want);
}

#[test]
fn a_cylinders_side_is_a_cylinder_of_its_radius_about_its_axis() {
    // r 2.5, h 10: the side a cylinder of radius 2.5 about the z axis (its direction as its
    // surface has it, of either sign: Silico's and FreeCAD's point −z), area
    // 2π · 2.5 · 10 = 50π, its centre of area (0, 0, 5); the caps planes of area 6.25π,
    // facing −z and +z; the rims circles of radius 2.5, length 5π, about +z or −z; the seam
    // a line 10 long.
    let d = described(Primitive::Cylinder {
        radius: 2.5,
        height: 10.0,
    });
    assert_eq!(d.faces.len(), 3);
    let mut caps = Vec::new();
    for face in &d.faces {
        match &face.surface {
            Surface::Cylinder { axis, radius } => {
                assert_eq!(*radius, 2.5);
                assert_point(
                    "the side's axis",
                    [axis.origin[0], axis.origin[1], 0.0],
                    [0.0; 3],
                );
                assert_point(
                    "the side's direction",
                    axis.direction.map(f64::abs),
                    [0.0, 0.0, 1.0],
                );
                assert_close("the side's area", face.area, 50.0 * PI, 1e-12);
                assert_point("the side's centre", face.centroid, [0.0, 0.0, 5.0]);
            }
            Surface::Plane { normal } => {
                assert_close("a cap's area", face.area, 6.25 * PI, 1e-12);
                caps.push((normal[2], face.centroid));
            }
            other => panic!("a cylinder has no {other:?}"),
        }
    }
    caps.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(caps.len(), 2);
    assert_point(
        "the bottom's normal",
        [0.0, 0.0, caps[0].0],
        [0.0, 0.0, -1.0],
    );
    assert_point("the bottom's centre", caps[0].1, [0.0; 3]);
    assert_point("the top's normal", [0.0, 0.0, caps[1].0], [0.0, 0.0, 1.0]);
    assert_point("the top's centre", caps[1].1, [0.0, 0.0, 10.0]);
    let (mut rims, mut seams) = (0, 0);
    for e in &d.edges {
        match &e.curve {
            EdgeCurve::Circle { axis, radius } => {
                rims += 1;
                assert_eq!(*radius, 2.5);
                assert_close("a rim's length", e.length, 5.0 * PI, 1e-12);
                assert_eq!(axis.direction[2].abs(), 1.0);
                assert!(axis.origin[2] == 0.0 || axis.origin[2] == 10.0);
            }
            EdgeCurve::Line => {
                seams += 1;
                assert_close("the seam's length", e.length, 10.0, 1e-12);
            }
            other => panic!("a cylinder has no {other:?}"),
        }
    }
    assert_eq!((rims, seams), (2, 1));
}

#[test]
fn a_spheres_poles_are_degenerated_edges_of_no_length() {
    // r 3: one spherical face about the origin, area 4π · 9 = 36π, its centre of area
    // the centre; its two poles degenerated edges at (0, 0, ±3), of length 0, and its seam
    // a half circle of length 3π.
    let d = described(Primitive::Sphere { radius: 3.0 });
    assert_eq!(d.faces.len(), 1);
    match &d.faces[0].surface {
        Surface::Sphere { axis, radius } => {
            assert_eq!(*radius, 3.0);
            assert_point("the centre", axis.origin, [0.0; 3]);
        }
        other => panic!("{other:?}"),
    }
    assert_close("the area", d.faces[0].area, 36.0 * PI, 1e-12);
    assert_point("the centre of area", d.faces[0].centroid, [0.0; 3]);
    let poles: Vec<_> = d.edges.iter().filter(|e| e.degenerated).collect();
    assert_eq!(poles.len(), 2);
    let mut heights: Vec<f64> = poles
        .iter()
        .map(|e| {
            assert_eq!((e.length, &e.curve), (0.0, &EdgeCurve::Other));
            assert_eq!(e.start, e.end);
            assert_eq!(e.start, e.middle);
            e.start[2]
        })
        .collect();
    heights.sort_by(f64::total_cmp);
    assert_point("the poles", [heights[0], heights[1], 0.0], [-3.0, 3.0, 0.0]);
    let seam: Vec<_> = d.edges.iter().filter(|e| !e.degenerated).collect();
    assert_eq!(seam.len(), 1);
    assert_close("the seam's length", seam[0].length, 3.0 * PI, 1e-12);
}

#[test]
fn a_cones_side_has_its_half_angle_and_its_apex_where_its_radius_would_be_none() {
    // r 4 to 0, h 6: its side a cone whose half-angle is atan(4/6) in size (narrowing up
    // its axis, so OCCT's is negative), of radius 4 at its axis's origin (0, 0, 0), so its
    // apex, origin − direction · radius / tan(half-angle), is (0, 0, 6); its area
    // π · 4 · √(4² + 6²); its base a plane of area 16π facing −z; its apex a degenerated
    // edge.
    let d = described(Primitive::Cone {
        radius1: 4.0,
        radius2: 0.0,
        height: 6.0,
    });
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
    assert_close(
        "the half-angle",
        half_angle.abs(),
        libm::atan(4.0 / 6.0),
        1e-12,
    );
    assert_eq!(*radius, 4.0);
    let apex =
        [0, 1, 2].map(|k| axis.origin[k] - axis.direction[k] * radius / libm::tan(*half_angle));
    assert_point("the apex", apex, [0.0, 0.0, 6.0]);
    assert_close(
        "the side's area",
        side.area,
        PI * 4.0 * libm::sqrt(52.0),
        1e-12,
    );
    let base = d
        .faces
        .iter()
        .find(|f| matches!(f.surface, Surface::Plane { .. }))
        .unwrap();
    assert_eq!(
        base.surface,
        Surface::Plane {
            normal: [0.0, 0.0, -1.0]
        }
    );
    assert_close("the base's area", base.area, 16.0 * PI, 1e-12);
    let apexes: Vec<_> = d.edges.iter().filter(|e| e.degenerated).collect();
    assert_eq!(apexes.len(), 1);
    assert_point("the degenerated edge", apexes[0].start, [0.0, 0.0, 6.0]);
}

#[test]
fn a_toruss_face_has_its_two_radii_and_area() {
    // R 10, r 2: one toroidal face about the z axis (either way along it), its radii 10
    // and 2, its area 4π²Rr = 80π².
    let d = described(Primitive::Torus {
        major_radius: 10.0,
        minor_radius: 2.0,
    });
    assert_eq!(d.faces.len(), 1);
    match &d.faces[0].surface {
        Surface::Torus {
            axis,
            major_radius,
            minor_radius,
        } => {
            assert_eq!((*major_radius, *minor_radius), (10.0, 2.0));
            assert_point("the axis's origin", axis.origin, [0.0; 3]);
            assert_point("the axis", axis.direction.map(f64::abs), [0.0, 0.0, 1.0]);
        }
        other => panic!("{other:?}"),
    }
    assert_close("the area", d.faces[0].area, 80.0 * PI * PI, 1e-12);
}

#[test]
fn faces_a_cut_turns_round_face_out_of_the_solid() {
    // A 10 mm cube less a 2 × 2 × 20 bar through it at (4, 4, −5): its hole's four faces,
    // at x = 4 and 6 and y = 4 and 6, face into the hole (x = 4's +x, x = 6's −x), and
    // its outside faces out. A point of each plane (its centre of area, or (1, 1) on the
    // top and bottom, whose centres are in the hole) moved 0.01 along its normal is
    // outside the solid, and 0.01 against it inside. The top's and bottom's areas are
    // 100 − 4 = 96.
    let cube = make(&Primitive::Box {
        dx: 10.0,
        dy: 10.0,
        dz: 10.0,
    })
    .unwrap();
    let bar = make(&Primitive::Box {
        dx: 2.0,
        dy: 2.0,
        dz: 20.0,
    })
    .unwrap();
    let bar = translate(&bar, [4.0, 4.0, -5.0]).unwrap().shape;
    let cut = boolean(BooleanOp::Cut, &cube, &bar).unwrap().shape;
    let d = describe(&cut).unwrap();
    d.check().unwrap();
    let inside = |p: [f64; 3]| {
        let in_cube = p.iter().all(|&c| (0.0..=10.0).contains(&c));
        let in_hole = (4.0..=6.0).contains(&p[0]) && (4.0..=6.0).contains(&p[1]);
        in_cube && !in_hole
    };
    assert_eq!(d.faces.len(), 10);
    let mut hole = 0;
    for (i, face) in d.faces.iter().enumerate() {
        let Surface::Plane { normal } = face.surface else {
            panic!("Face{} isn't a plane", i + 1)
        };
        let flat = normal[2] != 0.0;
        let on = if flat {
            [1.0, 1.0, face.centroid[2]]
        } else {
            face.centroid
        };
        let at = |t: f64| [0, 1, 2].map(|k| on[k] + t * normal[k]);
        assert!(!inside(at(0.01)), "Face{} faces in: {normal:?}", i + 1);
        assert!(inside(at(-0.01)), "Face{} faces in: {normal:?}", i + 1);
        if flat {
            assert_close("the top's or bottom's area", face.area, 96.0, 1e-12);
        } else if (4.0..=6.0).contains(&face.centroid[0]) && (4.0..=6.0).contains(&face.centroid[1])
        {
            hole += 1;
            assert_close("a hole face's area", face.area, 20.0, 1e-12);
        }
    }
    assert_eq!(hole, 4);
}
