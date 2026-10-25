//! `RenderScene::check` and `Camera::check` (docs/design/m4.6-render.md, The protocol): a
//! scene the worker can draw passes; each way one can't is refused, naming its field.

use silico_protocol::{
    Camera, EDGE_BIT, InvalidParameter, MAX_RENDER_SIDE, Mesh, Projection, RenderBody, RenderScene,
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
    s.camera.projection = Projection::Perspective {
        field_of_view: 179.999_999,
    };
    s.check().unwrap();
    s.camera.projection = Projection::Perspective {
        field_of_view: 1e-300,
    };
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
        assert_eq!(s.camera.check().unwrap_err().parameter, field, "case {i}");
    }
}

#[test]
fn bodies_numbered_0_or_out_of_order_are_refused() {
    for numbers in [[0, 4], [4, 4], [4, 1]] {
        let mut s = scene();
        s.bodies[0].body = numbers[0];
        s.bodies[1].body = numbers[1];
        let (parameter, message) = refused(&s);
        assert_eq!(parameter, "bodies");
        assert!(
            message.contains(&numbers[0].max(numbers[1]).to_string()) || message.contains('0'),
            "{message}"
        );
    }
}

#[test]
fn a_mesh_the_worker_couldnt_index_or_draw_is_refused_naming_what_breaks() {
    let cases: Vec<MeshCase> = vec![
        (
            |m| m.normals.pop().map(|_| ()).unwrap_or(()),
            "3 normals for 4 positions",
        ),
        (|m| m.positions[2][1] = f64::NAN, "position 2 isn't finite"),
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
            "edge point 4 isn't finite",
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
