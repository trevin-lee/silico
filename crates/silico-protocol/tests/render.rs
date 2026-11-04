//! `RenderScene::check` and `Camera::check` (docs/design/m4.6-render.md, The protocol): a
//! scene the worker can draw passes; each way one can't is refused, naming its field.

use silico_protocol::{
    Camera, EDGE_BIT, InvalidParameter, MAX_RENDER_COORDINATE, MAX_RENDER_EDGES, MAX_RENDER_FACES,
    MAX_RENDER_SIDE, MAX_RENDER_TRIANGLES, MIN_EYE_DISTANCE, Mesh, Projection, RenderBody,
    RenderScene, field_of_view_range,
};

/// A way to break a camera or a mesh, and what its refusal names.
type CameraCase = (fn(&mut Camera), &'static str);
type MeshCase = (fn(&mut Mesh), &'static str);

/// A unit square in z = 0, two triangles, as one face, with its four edges.
fn square() -> Mesh {
    Mesh {
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        face_triangles: vec![2],
        edge_points: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
        ],
        edge_point_counts: vec![2, 1, 1, 1],
        valid: None,
    }
}

fn scene() -> RenderScene {
    RenderScene {
        bodies: vec![
            RenderBody {
                body: 1,
                mesh: square(),
                highlight_faces: vec![0],
                highlight_edges: vec![0, 3],
            },
            RenderBody {
                body: 4,
                mesh: square(),
                highlight_faces: vec![],
                highlight_edges: vec![],
            },
        ],
        camera: Camera {
            projection: Projection::Orthographic { view_height: 2.0 },
            eye: [0.5, 0.5, 10.0],
            target: [0.5, 0.5, 0.0],
            up: [0.0, 1.0, 0.0],
        },
        width: 1,
        height: MAX_RENDER_SIDE,
        ids: true,
    }
}

/// The parameter `scene`'s check refuses, and its message, which must name the parameter.
fn refused(scene: &RenderScene) -> (&'static str, String) {
    let InvalidParameter { parameter, message } = scene.check().unwrap_err();
    assert!(message.contains(parameter), "{parameter}: {message}");
    (parameter, message)
}

#[test]
fn a_scene_the_worker_can_draw_passes_at_its_limits() {
    scene().check().unwrap();
    let mut s = scene();
    s.width = MAX_RENDER_SIDE;
    s.height = 1;
    s.bodies.clear();
    s.check().unwrap();
    // The least up across the view still passes.
    s.camera.up = [0.0, 1e-9, -1.0];
    s.check().unwrap();
    // A perspective view at either end of its angles at this size (4096 by 1).
    let (narrowest, widest) = field_of_view_range(s.width, s.height);
    for field_of_view in [narrowest, widest] {
        s.camera.projection = Projection::Perspective { field_of_view };
        s.check().unwrap();
    }
    // An eye as far out as a scene's point may be.
    s.camera.eye = [-MAX_RENDER_COORDINATE, 0.5, MAX_RENDER_COORDINATE];
    s.check().unwrap();
    // A face with no triangles (a sliver OCCT couldn't mesh) and an edge of one point.
    let mut s = scene();
    s.bodies[0].mesh.face_triangles = vec![2, 0];
    s.bodies[0].highlight_faces = vec![1];
    s.check().unwrap();
}

#[test]
fn sizes_outside_1_to_4096_are_refused_naming_the_side() {
    for (width, height, side) in [
        (0, 1, "width"),
        (MAX_RENDER_SIDE + 1, 1, "width"),
        (1, 0, "height"),
        (1, MAX_RENDER_SIDE + 1, "height"),
        (u32::MAX, 1, "width"),
    ] {
        let mut s = scene();
        s.width = width;
        s.height = height;
        assert_eq!(refused(&s).0, side, "{width} x {height}");
    }
}

#[test]
fn a_camera_that_cant_make_a_view_is_refused_naming_its_field() {
    let cases: Vec<CameraCase> = vec![
        (|c| c.eye[0] = f64::NAN, "camera.eye"),
        (|c| c.eye[2] = f64::INFINITY, "camera.eye"),
        (|c| c.eye[2] = 1.000_000_000_001e12, "camera.eye"),
        (|c| c.target[0] = -1e13, "camera.target"),
        (|c| c.target[1] = f64::NAN, "camera.target"),
        (|c| c.up[2] = f64::NEG_INFINITY, "camera.up"),
        (|c| c.eye = c.target, "camera.eye"),
        // Apart by more than f64 can hold: the view's length overflows.
        (
            |c| {
                c.eye = [-1e308, 0.0, 0.0];
                c.target = [1e308, 0.0, 0.0];
            },
            "camera.eye",
        ),
        (|c| c.up = [0.0; 3], "camera.up"),
        (|c| c.up = [0.0, 0.0, -3.0], "camera.up"),
        (|c| c.up = [0.0, 1e-10, -1.0], "camera.up"),
        (
            |c| c.projection = Projection::Orthographic { view_height: 0.0 },
            "camera.view_height",
        ),
        (
            |c| {
                c.projection = Projection::Orthographic {
                    view_height: 9.999e-7,
                }
            },
            "camera.view_height",
        ),
        (
            |c| {
                c.projection = Projection::Orthographic {
                    view_height: 1.0001e12,
                }
            },
            "camera.view_height",
        ),
        (
            |c| {
                c.projection = Projection::Perspective {
                    field_of_view: 179.9991,
                }
            },
            "camera.field_of_view",
        ),
        (
            |c| {
                c.projection = Projection::Perspective {
                    field_of_view: 9.99e-7,
                }
            },
            "camera.field_of_view",
        ),
        (
            |c| c.projection = Projection::Orthographic { view_height: -1.0 },
            "camera.view_height",
        ),
        (
            |c| {
                c.projection = Projection::Orthographic {
                    view_height: f64::INFINITY,
                }
            },
            "camera.view_height",
        ),
        (
            |c| {
                c.projection = Projection::Orthographic {
                    view_height: f64::NAN,
                }
            },
            "camera.view_height",
        ),
        (
            |c| c.projection = Projection::Perspective { field_of_view: 0.0 },
            "camera.field_of_view",
        ),
        (
            |c| {
                c.projection = Projection::Perspective {
                    field_of_view: 180.0,
                }
            },
            "camera.field_of_view",
        ),
        (
            |c| {
                c.projection = Projection::Perspective {
                    field_of_view: f64::NAN,
                }
            },
            "camera.field_of_view",
        ),
    ];
    for (i, (break_it, field)) in cases.into_iter().enumerate() {
        let mut s = scene();
        break_it(&mut s.camera);
        assert_eq!(refused(&s).0, field, "case {i}");
        assert_eq!(
            s.camera.check(s.width, s.height).unwrap_err().parameter,
            field,
            "case {i}"
        );
    }
}

#[test]
fn bodies_numbered_0_or_out_of_order_are_refused() {
    // Each names the body at fault, its number, and the one before it.
    for (numbers, says) in [
        (
            [0, 4],
            "bodies[0]'s number 0 must be at least 1 and above the one before it (0)",
        ),
        (
            [4, 4],
            "bodies[1]'s number 4 must be at least 1 and above the one before it (4)",
        ),
        (
            [4, 1],
            "bodies[1]'s number 1 must be at least 1 and above the one before it (4)",
        ),
    ] {
        let mut s = scene();
        s.bodies[0].body = numbers[0];
        s.bodies[1].body = numbers[1];
        let (parameter, message) = refused(&s);
        assert_eq!(parameter, "bodies");
        assert_eq!(message, says);
    }
}

#[test]
fn an_eye_within_a_nanometre_of_the_target_is_refused_and_one_at_it_passes() {
    let mut s = scene();
    s.camera.eye = [0.5, 0.5, MIN_EYE_DISTANCE];
    s.camera.target = [0.5, 0.5, 0.0];
    s.check().unwrap();
    s.camera.eye = [0.5, 0.5, MIN_EYE_DISTANCE * 0.999];
    let (parameter, message) = refused(&s);
    assert_eq!(parameter, "camera.eye");
    assert!(
        message.contains("must be at least 0.000001 mm from camera.target"),
        "{message}"
    );
}

#[test]
fn a_scene_past_the_most_faces_or_edges_a_render_draws_is_refused() {
    // MAX_RENDER_FACES and MAX_RENDER_EDGES across the bodies: at them it passes, one past
    // is refused. Faces with no triangles and edges with no points count.
    let square = |faces: usize, edges: usize| {
        let mut m = square();
        m.face_triangles = vec![0; faces];
        m.face_triangles[0] = 2;
        m.edge_point_counts = vec![0; edges];
        m.edge_point_counts[0] = 5;
        m
    };
    let mut s = scene();
    s.bodies[0].mesh = square(MAX_RENDER_FACES - 1, 1);
    s.bodies[0].highlight_faces.clear();
    s.bodies[0].highlight_edges.clear();
    s.bodies[1].mesh = square(1, MAX_RENDER_EDGES - 1);
    s.check().unwrap();
    s.bodies[1].mesh = square(2, MAX_RENDER_EDGES - 1);
    let (parameter, message) = refused(&s);
    assert_eq!(parameter, "bodies");
    assert_eq!(
        message,
        format!(
            "the bodies have {} faces, more than the {MAX_RENDER_FACES} a render draws within its time",
            MAX_RENDER_FACES + 1
        )
    );
    s.bodies[1].mesh = square(1, MAX_RENDER_EDGES);
    let (_, message) = refused(&s);
    assert!(
        message.starts_with(&format!("the bodies have {} edges", MAX_RENDER_EDGES + 1)),
        "{message}"
    );
}

#[test]
fn a_scene_past_the_most_triangles_a_render_draws_is_refused() {
    // MAX_RENDER_TRIANGLES across the bodies: at it passes, one past is refused.
    let mut s = scene();
    let half = MAX_RENDER_TRIANGLES / 2;
    for b in &mut s.bodies {
        b.mesh.triangles = vec![[0, 1, 2]; half];
        b.mesh.face_triangles = vec![u32::try_from(half).unwrap()];
        b.highlight_faces.clear();
    }
    s.check().unwrap();
    s.bodies[1].mesh.triangles.push([0, 2, 3]);
    s.bodies[1].mesh.face_triangles[0] += 1;
    let (parameter, message) = refused(&s);
    assert_eq!(parameter, "bodies");
    assert_eq!(
        message,
        format!(
            "the bodies have {} triangles, more than the {MAX_RENDER_TRIANGLES} a render draws within its time",
            MAX_RENDER_TRIANGLES + 1
        )
    );
}

#[test]
fn an_orthographic_views_least_height_counts_edge_points_too() {
    // The bodies reach from the target to their farthest point, an edge's as well as a
    // mesh's: an edge point 1e5 from the target needs a view at 4096 rows at least 4096 ×
    // 1e5 / 2^19 = 781.25 high (by hand, exact); the squares alone reach 0.71, 0.0055 high.
    let mut s = scene();
    s.camera.projection = Projection::Orthographic {
        view_height: 781.25f64.next_down(),
    };
    s.check().unwrap();
    s.bodies[1].mesh.edge_points[0] = [0.5, 0.5 + 1e5, 0.0];
    let (parameter, message) = refused(&s);
    assert_eq!(parameter, "camera.view_height");
    assert!(
        message.contains(
            "is too small for an image 4096 pixels high of bodies that reach 100000.0 mm from camera.target: at least 781.25 mm"
        ),
        "{message}"
    );
    s.camera.projection = Projection::Orthographic {
        view_height: 781.25,
    };
    s.check().unwrap();
}

#[test]
fn a_mesh_the_worker_couldnt_index_or_draw_is_refused_naming_what_breaks() {
    let cases: Vec<MeshCase> = vec![
        (
            |m| m.normals.pop().map(|_| ()).unwrap_or(()),
            "3 normals for 4 positions",
        ),
        (
            |m| m.positions[2][1] = f64::NAN,
            "position 2 must be finite",
        ),
        (
            |m| m.positions[1][0] = 1.000_000_000_001e12,
            "position 1 must be finite and within 1000000000000 mm",
        ),
        (|m| m.normals[3][0] = f32::INFINITY, "normal 3 isn't finite"),
        (
            |m| m.triangles[1][2] = 4,
            "triangle 1 indexes past its 4 positions",
        ),
        (
            |m| m.triangles[0][0] = u32::MAX,
            "triangle 0 indexes past its 4 positions",
        ),
        (
            |m| m.face_triangles = vec![1],
            "its faces count 1 triangles, and it has 2",
        ),
        (
            |m| m.face_triangles = vec![u32::MAX, u32::MAX, 4],
            "its faces count",
        ),
        (
            |m| m.edge_points[4][2] = f64::NAN,
            "edge point 4 must be finite",
        ),
        (
            |m| m.edge_point_counts = vec![2, 1, 1],
            "its edges count 4 points, and it has 5",
        ),
    ];
    for (break_it, says) in cases {
        let mut s = scene();
        break_it(&mut s.bodies[1].mesh);
        let (parameter, message) = refused(&s);
        assert_eq!(parameter, "bodies");
        assert!(
            message.contains("bodies[1]") && message.contains(says),
            "{says}: {message}"
        );
    }
}

#[test]
fn highlights_out_of_order_repeated_or_past_the_mesh_are_refused() {
    for (faces, edges) in [
        (vec![1], vec![]),
        (vec![0, 0], vec![]),
        (vec![], vec![3, 0]),
        (vec![], vec![4]),
        (vec![], vec![u32::MAX]),
        (vec![], vec![EDGE_BIT]),
    ] {
        let mut s = scene();
        s.bodies[0].highlight_faces = faces.clone();
        s.bodies[0].highlight_edges = edges.clone();
        let (parameter, message) = refused(&s);
        assert_eq!(parameter, "bodies");
        let field = if faces.is_empty() {
            "highlight_edges"
        } else {
            "highlight_faces"
        };
        assert!(
            message.contains(field) && message.contains("bodies[0]"),
            "{faces:?} {edges:?}: {message}"
        );
    }
}

#[test]
fn the_adapter_policy_reads_back_as_written_and_nothing_else() {
    use silico_protocol::RenderAdapter;
    use std::ffi::OsString;
    let read = |args: &[&str]| RenderAdapter::from_args(args.iter().map(OsString::from));
    for policy in [RenderAdapter::Software, RenderAdapter::Any] {
        assert_eq!(read(policy.args()), Ok(policy), "{policy:?}");
    }
    // By hand: no argument is lavapipe only; the one argument is any adapter.
    assert_eq!(RenderAdapter::Software.args(), &[] as &[&str]);
    assert_eq!(RenderAdapter::Any.args(), &["--any-adapter"]);
    assert_eq!(RenderAdapter::default(), RenderAdapter::Software);
    for bad in [
        &["--any"][..],
        &["--any-adapter", "--any-adapter"],
        &["--Any-adapter"],
        &["--any-adapter", "x"],
        &[""],
    ] {
        let e = read(bad).unwrap_err();
        assert!(e.contains("unexpected argument"), "{bad:?}: {e}");
    }
}

#[test]
fn a_meshs_floor_is_a_ten_thousandth_of_its_diagonal_and_never_under_the_tolerance() {
    use silico_protocol::mesh_floor;
    // A 100 mm cube's diagonal, 173.205...: 0.0173205...
    let diagonal = 100.0 * 3.0f64.sqrt();
    assert!((mesh_floor(diagonal) - 0.017_320_508_075_688_77).abs() < 1e-15);
    assert_eq!(mesh_floor(1.0), 1e-4);
    // Under 1e-3 mm, the length tolerance; over it, a ten-thousandth (1e-3 × 1e-4 rounds
    // an ulp over 1e-7, so it's the product's).
    assert_eq!(mesh_floor(5e-4), 1e-7);
    assert_eq!(mesh_floor(1e-3f64.next_down()), 1e-7);
    assert_eq!(mesh_floor(1e-3), 1e-7f64.next_up());
    assert!((mesh_floor(2e-3) - 2e-7).abs() < 1e-22);
    assert_eq!(mesh_floor(0.0), 1e-7);
}

#[test]
fn a_classified_point_is_finite_and_within_a_render_of_the_origin() {
    use silico_protocol::{MAX_CLASSIFY_COORDINATE, check_classify_point};
    assert_eq!(MAX_CLASSIFY_COORDINATE, 1e12);
    for ok in [
        [1e12, 0.0, 0.0],
        [0.0, -1e12, 1e12],
        [0.0; 3],
        [-0.0, 5e-324, 1.0],
    ] {
        assert_eq!(check_classify_point(ok), Ok(()), "{ok:?}");
    }
    for bad in [
        [1e12f64.next_up(), 0.0, 0.0],
        [0.0, 0.0, -1e12f64.next_up()],
        [f64::NAN, 0.0, 0.0],
        [0.0, f64::INFINITY, 0.0],
        [0.0, 0.0, f64::NEG_INFINITY],
    ] {
        let e = check_classify_point(bad).unwrap_err();
        assert_eq!(e.parameter, "point", "{bad:?}");
        assert!(
            e.message.contains("finite and within 1000000000000 mm"),
            "{}",
            e.message
        );
    }
}

#[test]
fn a_cameras_frame_makes_a_long_tilted_up_perpendicular_and_of_unit_length() {
    // By hand: the eye at (0, −3, 4) from the origin, so back is (0, −0.6, 0.8). Up (0, 0, 2)
    // has 1.6 along it; less that, (0, 0.96, 0.72), 1.2 long, so up is (0, 0.8, 0.6).
    let camera = Camera {
        projection: Projection::Orthographic { view_height: 10.0 },
        eye: [0.0, -3.0, 4.0],
        target: [0.0; 3],
        up: [0.0, 0.0, 2.0],
    };
    let frame = camera.frame().unwrap();
    for (got, want) in frame.back.iter().zip([0.0, -0.6, 0.8]) {
        assert!((got - want).abs() < 1e-15, "back {:?}", frame.back);
    }
    for (got, want) in frame.up.iter().zip([0.0, 0.8, 0.6]) {
        assert!((got - want).abs() < 1e-15, "up {:?}", frame.up);
    }
    assert!((frame.distance - 5.0).abs() < 1e-15);
}

/// One body, numbered `body`: a triangle of `points`, one face, no edges.
fn triangle(body: u32, points: [[f64; 3]; 3]) -> RenderBody {
    RenderBody {
        body,
        mesh: Mesh {
            positions: points.to_vec(),
            normals: vec![[0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2]],
            face_triangles: vec![1],
            edge_points: vec![],
            edge_point_counts: vec![],
            valid: None,
        },
        highlight_faces: vec![],
        highlight_edges: vec![],
    }
}

#[test]
fn an_orthographic_views_least_height_is_its_rows_times_the_reach_over_2_to_the_19() {
    // By hand, exact (a power of two's scaling): at 1 row, bodies reaching 2^19 mm from the
    // target need a view 1 mm high; at 2048 rows, the 679401.3681066748 mm slab from its
    // corner needs 679401.3681066748 / 256 = 2653.911_594_166_698_3 mm. Each passes there
    // and is refused a double under it, the refusal giving the least to the nanometre
    // above, which passes.
    for (width, height, reach, least, shown) in [
        (1, 1, 524_288.0, 1.0, "1"),
        (
            40,
            2048,
            679_401.368_106_674_8,
            2_653.911_594_166_698_4,
            "2653.911595",
        ),
    ] {
        let mut s = RenderScene {
            bodies: vec![triangle(
                2,
                [[reach, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            )],
            camera: Camera {
                projection: Projection::Orthographic { view_height: least },
                eye: [0.0, -10.0, 0.0],
                target: [0.0; 3],
                up: [0.0, 0.0, 1.0],
            },
            width,
            height,
            ids: false,
        };
        s.check().unwrap();
        s.camera.projection = Projection::Orthographic {
            view_height: least.next_down(),
        };
        let (parameter, message) = refused(&s);
        assert_eq!(parameter, "camera.view_height");
        assert!(
            message.contains(&format!(
                "for an image {height} pixels high of bodies that reach {reach:?} mm from camera.target: at least {shown} mm"
            )),
            "{message}"
        );
        s.camera.projection = Projection::Orthographic {
            view_height: shown.parse().unwrap(),
        };
        s.check().unwrap();
    }
}

#[test]
fn a_perspective_view_takes_bodies_behind_at_and_far_beside_its_eye() {
    // What a perspective view draws is cut to its pyramid as it's drawn (docs/design/m4.6-
    // render.md, *A given camera*), so the check takes bodies wherever they are: behind the
    // eye's plane, through the eye, 1e6 times as far across the view as along it.
    let scene = |points: [[f64; 3]; 3]| RenderScene {
        bodies: vec![triangle(3, points)],
        camera: Camera {
            projection: Projection::Perspective { field_of_view: 0.5 },
            eye: [0.0; 3],
            target: [0.0, 0.0, -10.0],
            up: [0.0, 1.0, 0.0],
        },
        width: 40,
        height: 2048,
        ids: false,
    };
    for points in [
        [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]],
        [[0.0, 0.0, 0.0], [1.0, 0.0, -1.0], [0.0, 1.0, -1.0]],
        [[1e6, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 0.1, -1.0]],
    ] {
        scene(points).check().unwrap();
    }
}

#[test]
fn an_orthographic_views_corners_bound_its_reach_and_the_least_is_its_points() {
    use silico_protocol::{points_box, reach_of_box, reach_of_points};
    // The triangle seen orthographic about (0, 1, 10), its vertex: its farthest vertex,
    // (0, 11, 0), is √200 off; its box's farthest corner, (1, 11, 0), √201. At 48 rows the
    // least height is the points', 48 √200 / 2^19 (by hand), which the corners alone would
    // refuse: taken there, and refused a double under it, naming the points' reach.
    let mut s = RenderScene {
        bodies: vec![triangle(
            7,
            [[0.0, 1.0, 10.0], [0.0, 11.0, 0.0], [1.0, 6.0, 5.0]],
        )],
        camera: Camera {
            projection: Projection::Orthographic { view_height: 1.0 },
            eye: [-10.0, 1.0, 10.0],
            target: [0.0, 1.0, 10.0],
            up: [0.0, 0.0, 1.0],
        },
        width: 64,
        height: 48,
        ids: false,
    };
    s.camera = Camera {
        projection: Projection::Orthographic { view_height: 1.0 },
        eye: [-10.0, 1.0, 10.0],
        target: [0.0, 1.0, 10.0],
        up: [0.0, 0.0, 1.0],
    };
    let mesh = &s.bodies[0].mesh;
    assert_eq!(reach_of_points(mesh, s.camera.target), 200f64.sqrt());
    assert_eq!(
        reach_of_box(points_box(mesh).unwrap(), s.camera.target),
        201f64.sqrt()
    );
    let least = 48.0 * 200f64.sqrt() / 524_288.0;
    s.camera.projection = Projection::Orthographic { view_height: least };
    s.check().unwrap();
    s.camera.projection = Projection::Orthographic {
        view_height: least.next_down(),
    };
    let (parameter, message) = refused(&s);
    assert_eq!(parameter, "camera.view_height");
    assert!(
        message.contains(&format!("of bodies that reach {:?} mm", 200f64.sqrt())),
        "{message}"
    );
}
