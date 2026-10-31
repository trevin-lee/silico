//! M4.4: `describe`, each element's geometry and measures (docs/design/m4.4-describe.md),
//! against closed forms: a box's faces, edges, and corners; a cylinder's side, caps, rims,
//! and seam; a sphere's and a cone's degenerated edges; a torus's radii; and the normals of
//! faces a cut reverses, which point out of the solid.

use super::close;

use std::f64::consts::PI;

use close::assert_close;
use silico_kernel::{BooleanOp, Primitive, boolean, describe, describe_whole, make, translate};
use silico_protocol::ElementKind;
use silico_protocol::{Description, EdgeCurve, Surface};

fn described(p: Primitive) -> Description {
    let d = describe_whole(&make(&p).unwrap()).unwrap();
    d.validate(1).unwrap();
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
        let Surface::Plane {
            normal: got,
            origin,
        } = face.surface
        else {
            panic!("{what}: {:?}", face.surface)
        };
        assert_eq!(got, normal, "{what}");
        // The plane's own location is on it: along the normal, its offset exactly (0, 10,
        // 0, 20, 0, 30 out from the origin).
        let offset: f64 = (0..3).map(|k| normal[k] * origin[k]).sum();
        assert_eq!(
            offset,
            [0.0, 10.0, 0.0, 20.0, 0.0, 30.0][i],
            "{what}: {origin:?}"
        );
        assert_close(&what, face.area, area, 1e-12);
        assert_point(&what, face.centroid, centroid);
    }
}

#[test]
fn a_moved_boxs_planes_are_located_on_themselves_where_the_box_was_moved_to() {
    // The 10 × 20 × 30 box moved to (3, 4, 5): its faces at x = 3 and 13, y = 4 and 24,
    // z = 5 and 35. Each plane's own location is on it, the move applied: along the
    // outward normal, exactly the face's offset from the origin (−3, 13, −4, 24, −5, 35),
    // where the box unmoved gives 0, 10, 0, 20, 0, 30.
    let moved = translate(
        &make(&Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap(),
        [3.0, 4.0, 5.0],
    )
    .unwrap()
    .shape;
    let d = describe_whole(&moved).unwrap();
    d.validate(1).unwrap();
    let want = [
        ([-1.0, 0.0, 0.0], -3.0),
        ([1.0, 0.0, 0.0], 13.0),
        ([0.0, -1.0, 0.0], -4.0),
        ([0.0, 1.0, 0.0], 24.0),
        ([0.0, 0.0, -1.0], -5.0),
        ([0.0, 0.0, 1.0], 35.0),
    ];
    assert_eq!(d.faces.len(), 6);
    for (i, (face, (normal, offset))) in d.faces.iter().zip(want).enumerate() {
        let what = format!("Face{}", i + 1);
        let Surface::Plane {
            normal: got,
            origin,
        } = face.surface
        else {
            panic!("{what}: {:?}", face.surface)
        };
        assert_eq!(got, normal, "{what}");
        let along: f64 = (0..3).map(|k| normal[k] * origin[k]).sum();
        assert_eq!(along, offset, "{what}: {origin:?}");
        // Within the box's extent across the face too: a point of the face's plane near
        // the face, not one of the unmoved box's.
        for (k, (low, high)) in [(3.0, 13.0), (4.0, 24.0), (5.0, 35.0)]
            .into_iter()
            .enumerate()
        {
            assert!((low..=high).contains(&origin[k]), "{what}: {origin:?}");
        }
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
        assert!(e.curve != EdgeCurve::Degenerated);
        let span: f64 = (0..3)
            .map(|k| (e.end[k] - e.start[k]) * (e.end[k] - e.start[k]))
            .sum();
        assert_close(&what, e.length, libm::sqrt(span), 1e-12);
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
    // surface has it: −z, as FreeCAD's Part::Cylinder's), area
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
                // −z, as FreeCAD 1.1.3's Part::Cylinder has it: (−0, −0, −1), its face
                // reversed (Part.makeCylinder's is +z).
                assert_point("the side's direction", axis.direction, [0.0, 0.0, -1.0]);
                assert_close("the side's area", face.area, 50.0 * PI, 1e-12);
                assert_point("the side's centre", face.centroid, [0.0, 0.0, 5.0]);
            }
            Surface::Plane { normal, .. } => {
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
    let poles: Vec<_> = d
        .edges
        .iter()
        .filter(|e| e.curve == EdgeCurve::Degenerated)
        .collect();
    assert_eq!(poles.len(), 2);
    let mut heights: Vec<f64> = poles
        .iter()
        .map(|e| {
            assert_eq!((e.length, &e.curve), (0.0, &EdgeCurve::Degenerated));
            assert_eq!(e.start, e.end);
            assert_eq!(e.start, e.middle);
            e.start[2]
        })
        .collect();
    heights.sort_by(f64::total_cmp);
    assert_point("the poles", [heights[0], heights[1], 0.0], [-3.0, 3.0, 0.0]);
    let seam: Vec<_> = d
        .edges
        .iter()
        .filter(|e| e.curve != EdgeCurve::Degenerated)
        .collect();
    assert_eq!(seam.len(), 1);
    assert_close("the seam's length", seam[0].length, 3.0 * PI, 1e-12);
}

#[test]
fn a_cones_side_has_its_half_angle_and_its_apex_where_its_radius_would_be_none() {
    // r 4 to 0, h 6: its side a cone whose half-angle is atan(−4/6) (narrowing up its axis,
    // so negative), of radius 4 at its axis's origin (0, 0, 0), so its
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
    // −0.5880026035475675, narrowing up its axis, as FreeCAD 1.1.3's Part::Cone gives it.
    assert_close("the half-angle", *half_angle, libm::atan(-4.0 / 6.0), 1e-12);
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
    let Surface::Plane { normal, origin } = base.surface else {
        unreachable!()
    };
    assert_eq!(normal, [0.0, 0.0, -1.0]);
    // The base's plane's own location is on it, z = 0.
    assert_eq!(origin[2], 0.0, "{origin:?}");
    assert_close("the base's area", base.area, 16.0 * PI, 1e-12);
    let apexes: Vec<_> = d
        .edges
        .iter()
        .filter(|e| e.curve == EdgeCurve::Degenerated)
        .collect();
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
    let d = describe_whole(&cut).unwrap();
    d.validate(1).unwrap();
    let inside = |p: [f64; 3]| {
        let in_cube = p.iter().all(|&c| (0.0..=10.0).contains(&c));
        let in_hole = (4.0..=6.0).contains(&p[0]) && (4.0..=6.0).contains(&p[1]);
        in_cube && !in_hole
    };
    assert_eq!(d.faces.len(), 10);
    let mut hole = 0;
    for (i, face) in d.faces.iter().enumerate() {
        let Surface::Plane { normal, .. } = face.surface else {
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

#[test]
fn a_boxs_edges_and_vertices_are_freecads_by_number_each_edge_from_its_curves_start() {
    // A 10 × 20 × 30 box, as FreeCAD 1.1.3's Part::Box gives it (Shape.Edges[i] with
    // firstVertex() and lastVertex(), the curve's own order, and Shape.Vertexes): six of
    // its edges (3, 4, 5, 6, 10, and 11) are reversed in the faces OCCT meets them in first, but
    // each runs from its
    // curve's start, so an edit that reorders the faces can't swap its ends.
    let d = described(Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    });
    let ends = [
        ([0.0, 0.0, 0.0], [0.0, 0.0, 30.0]),
        ([0.0, 0.0, 30.0], [0.0, 20.0, 30.0]),
        ([0.0, 20.0, 0.0], [0.0, 20.0, 30.0]),
        ([0.0, 0.0, 0.0], [0.0, 20.0, 0.0]),
        ([10.0, 0.0, 0.0], [10.0, 0.0, 30.0]),
        ([10.0, 0.0, 30.0], [10.0, 20.0, 30.0]),
        ([10.0, 20.0, 0.0], [10.0, 20.0, 30.0]),
        ([10.0, 0.0, 0.0], [10.0, 20.0, 0.0]),
        ([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]),
        ([0.0, 0.0, 30.0], [10.0, 0.0, 30.0]),
        ([0.0, 20.0, 0.0], [10.0, 20.0, 0.0]),
        ([0.0, 20.0, 30.0], [10.0, 20.0, 30.0]),
    ];
    assert_eq!(d.edges.len(), 12);
    for (i, (e, (start, end))) in d.edges.iter().zip(ends).enumerate() {
        assert_eq!((e.start, e.end), (start, end), "Edge{}", i + 1);
    }
    assert_eq!(
        d.vertices,
        [
            [0.0, 0.0, 30.0],
            [0.0, 0.0, 0.0],
            [0.0, 20.0, 30.0],
            [0.0, 20.0, 0.0],
            [10.0, 0.0, 30.0],
            [10.0, 0.0, 0.0],
            [10.0, 20.0, 30.0],
            [10.0, 20.0, 0.0],
        ]
    );
}

#[test]
fn a_moved_spheres_centre_and_poles_move_with_it() {
    // r 3 moved to (5, −7, 2): its centre and centre of area there, its poles at
    // (5, −7, 2 ± 3).
    let sphere = make(&Primitive::Sphere { radius: 3.0 }).unwrap();
    let moved = translate(&sphere, [5.0, -7.0, 2.0]).unwrap().shape;
    let d = describe_whole(&moved).unwrap();
    d.validate(1).unwrap();
    match &d.faces[0].surface {
        Surface::Sphere { axis, radius } => {
            assert_eq!(*radius, 3.0);
            assert_point("the centre", axis.origin, [5.0, -7.0, 2.0]);
        }
        other => panic!("{other:?}"),
    }
    assert_point("the centre of area", d.faces[0].centroid, [5.0, -7.0, 2.0]);
    let mut poles: Vec<[f64; 3]> = d
        .edges
        .iter()
        .filter(|e| e.curve == EdgeCurve::Degenerated)
        .map(|e| e.start)
        .collect();
    poles.sort_by(|a, b| a[2].total_cmp(&b[2]));
    assert_eq!(poles.len(), 2);
    assert_point("the low pole", poles[0], [5.0, -7.0, -1.0]);
    assert_point("the high pole", poles[1], [5.0, -7.0, 5.0]);
}

#[test]
fn a_batch_from_the_middle_is_those_elements_by_the_kernels_numbers() {
    // The 10 × 20 × 30 box's edges 5 to 7, as FreeCAD 1.1.3 numbers them (the test above's
    // ends); its last edge alone; none past it, the total still 12; and none from 0, which
    // numbers nothing.
    let shape = make(&Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
    .unwrap();
    let (total, d) = describe(&shape, ElementKind::Edge, 5, 3).unwrap();
    assert_eq!(total, 12);
    let ends: Vec<_> = d.edges.iter().map(|e| (e.start, e.end)).collect();
    assert_eq!(
        ends,
        [
            ([10.0, 0.0, 0.0], [10.0, 0.0, 30.0]),
            ([10.0, 0.0, 30.0], [10.0, 20.0, 30.0]),
            ([10.0, 20.0, 0.0], [10.0, 20.0, 30.0]),
        ]
    );
    assert!(d.faces.is_empty() && d.vertices.is_empty());
    let (_, d) = describe(&shape, ElementKind::Edge, 12, 1000).unwrap();
    let ends: Vec<_> = d.edges.iter().map(|e| (e.start, e.end)).collect();
    assert_eq!(ends, [([0.0, 20.0, 30.0], [10.0, 20.0, 30.0])]);
    let (total, d) = describe(&shape, ElementKind::Edge, 13, 5).unwrap();
    assert_eq!((total, d.edges.len()), (12, 0));
    let from_0 = describe(&shape, ElementKind::Edge, 0, 5).unwrap_err();
    assert!(
        from_0.to_string().contains("elements are numbered from 1"),
        "{from_0}"
    );
}

#[test]
fn a_solid_of_more_than_a_batch_is_described_whole_each_element_once() {
    // A prism of a regular 340-gon (radius 10, 1 high): by hand, 342 faces (the sides and
    // the two caps), 1,020 edges (340 round each cap and 340 up the sides), and 680
    // vertices: the edges two batches. Every edge described once (no two alike, end to
    // end), in this process and through a worker.
    let corners = 340;
    let points = (0..corners)
        .map(|i| {
            let t = std::f64::consts::TAU * f64::from(i) / f64::from(corners);
            [10.0 * libm::cos(t), 10.0 * libm::sin(t), 0.0]
        })
        .collect();
    let built = silico_kernel::extrude(&silico_kernel::Profile { points }, 1.0, None).unwrap();
    let d = describe_whole(&built.shape).unwrap();
    assert_eq!(
        (d.faces.len(), d.edges.len(), d.vertices.len()),
        (342, 1020, 680)
    );
    let distinct = |d: &Description| {
        let ends: std::collections::BTreeSet<String> = d
            .edges
            .iter()
            .map(|e| format!("{:?}", (e.start, e.end)))
            .collect();
        ends.len()
    };
    assert_eq!(distinct(&d), 1020);
    let mut worker = silico_core::kernel::KernelWorker::start_worker(std::path::Path::new(env!(
        "CARGO_BIN_EXE_silico-kernel-worker"
    )))
    .unwrap();
    let data = silico_core::kernel::ShapeData::checked(
        silico_core::kernel::Blob::inline(silico_kernel::to_brep(&built.shape).unwrap()),
        true,
    );
    // Through a worker, which reads the shape from its B-rep: as the shape read back from
    // that B-rep describes in this process, to the bit.
    let through = worker.describe(&data).unwrap();
    let read = silico_kernel::from_brep(&silico_kernel::to_brep(&built.shape).unwrap()).unwrap();
    assert_eq!(through, describe_whole(&read).unwrap());
    assert_eq!(distinct(&through), 1020);
}

#[test]
fn a_solid_of_a_batch_or_fewer_is_described_all_at_once_as_its_batches_describe_it() {
    // A 10 × 20 × 30 box (6 faces, 12 edges, 8 vertices: 26) through a worker: one
    // `DescribeAll` gives the description its three batches give, to the bit.
    let shape = make(&Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
    .unwrap();
    let data = silico_core::kernel::ShapeData::checked(
        silico_core::kernel::Blob::inline(silico_kernel::to_brep(&shape).unwrap()),
        true,
    );
    let mut worker = silico_core::kernel::KernelWorker::start_worker(std::path::Path::new(env!(
        "CARGO_BIN_EXE_silico-kernel-worker"
    )))
    .unwrap();
    let all = worker
        .run(silico_core::kernel::KernelRequest::describe_all(&data))
        .unwrap();
    assert_eq!(
        (all.faces.len(), all.edges.len(), all.vertices.len()),
        (6, 12, 8)
    );
    assert_eq!(all, worker.describe(&data).unwrap());
}

#[test]
fn a_solid_of_exactly_a_batch_of_elements_is_described_all_at_once() {
    // A 165-gon prism (by hand 167 faces, 495 edges, 330 vertices: 992) and, apart from it,
    // a cylinder (3 faces, 3 edges, 2 vertices: 8) fused: 1,000 elements, the most one
    // request describes, described all at once as its batches describe it.
    let corners = 165;
    let points = (0..corners)
        .map(|i| {
            let t = std::f64::consts::TAU * f64::from(i) / f64::from(corners);
            [10.0 * libm::cos(t), 10.0 * libm::sin(t), 0.0]
        })
        .collect();
    let prism = silico_kernel::extrude(&silico_kernel::Profile { points }, 1.0, None)
        .unwrap()
        .shape;
    let cylinder = make(&Primitive::Cylinder {
        radius: 1.0,
        height: 1.0,
    })
    .unwrap();
    let cylinder = translate(&cylinder, [50.0, 0.0, 0.0]).unwrap().shape;
    let both = boolean(BooleanOp::Fuse, &prism, &cylinder).unwrap().shape;
    let data = silico_core::kernel::ShapeData::checked(
        silico_core::kernel::Blob::inline(silico_kernel::to_brep(&both).unwrap()),
        true,
    );
    let mut worker = silico_core::kernel::KernelWorker::start_worker(std::path::Path::new(env!(
        "CARGO_BIN_EXE_silico-kernel-worker"
    )))
    .unwrap();
    let all = worker
        .run(silico_core::kernel::KernelRequest::describe_all(&data))
        .unwrap();
    assert_eq!(
        (all.faces.len(), all.edges.len(), all.vertices.len()),
        (170, 498, 332)
    );
    assert_eq!(all, worker.describe(&data).unwrap());
}

#[test]
fn a_solid_of_more_than_a_batch_is_refused_all_at_once_before_any_is_described() {
    // The 340-gon prism: by hand 342 + 1,020 + 680 = 2,042 elements, past the 1,000 one
    // request describes. Refused as the worker's fault (the core never asks it of a shape
    // its summary counts past a batch), naming the count, and the worker goes on: its
    // batches still describe it.
    let corners = 340;
    let points = (0..corners)
        .map(|i| {
            let t = std::f64::consts::TAU * f64::from(i) / f64::from(corners);
            [10.0 * libm::cos(t), 10.0 * libm::sin(t), 0.0]
        })
        .collect();
    let built = silico_kernel::extrude(&silico_kernel::Profile { points }, 1.0, None).unwrap();
    let data = silico_core::kernel::ShapeData::checked(
        silico_core::kernel::Blob::inline(silico_kernel::to_brep(&built.shape).unwrap()),
        true,
    );
    let mut worker = silico_core::kernel::KernelWorker::start_worker(std::path::Path::new(env!(
        "CARGO_BIN_EXE_silico-kernel-worker"
    )))
    .unwrap();
    let refused = worker
        .run(silico_core::kernel::KernelRequest::describe_all(&data))
        .unwrap_err();
    assert_eq!(refused.code(), silico_core::ErrorCode::Internal);
    assert!(
        refused.cause().is_some_and(|cause| cause
            .contains("the shape has 2042 faces, edges, and vertices, more than the 1000")),
        "{refused}"
    );
    let d = worker.describe(&data).unwrap();
    assert_eq!(
        (d.faces.len(), d.edges.len(), d.vertices.len()),
        (342, 1020, 680)
    );
}
