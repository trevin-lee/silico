//! M5.5: `command::meshes` of a real solid, rebuilt and meshed on kernel workers
//! (docs/design/m5.0-app.md, `meshes`): a box's front-view display mesh, every expected value
//! by hand from the box (60 by 40 by 50 at the origin, in OCCT's box order: faces x = 0,
//! x = 60, y = 0, y = 40, z = 0, z = 50; twelve straight edges; eight corners, (0, 0, 50)
//! first).

use super::model::{checked, published};

use std::path::Path;
use std::sync::Arc;

use silico_core::command::meshes::{Meshes, MeshesOn, meshes};
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
use silico_core::pool::KernelPool;
use silico_core::recompute::Recompute;
use silico_protocol::{Camera, Projection};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

const BOX: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 60, "width": 40, "height": 50}
  ]
}
"#;

#[test]
fn a_boxs_front_view_mesh_holds_its_six_faces_two_triangles_each_named_as_the_element_map_names_them()
 {
    let pool = Arc::new(KernelPool::start_on_demand(Path::new(WORKER), 1).unwrap());
    let model = published(&Recompute::new(Arc::clone(&pool)), checked(BOX));
    let cache = MeshCache::new(MESH_CACHE_BYTES);
    // From −y onto the box's centre, 100 mm high over 1,000 pixels: a half pixel of 0.05,
    // so 2⁻⁵, above the box's floor (1e-4 of its diagonal, √7700: 0.0088).
    let camera = Camera {
        projection: Projection::Orthographic { view_height: 100.0 },
        eye: [30.0, -200.0, 25.0],
        target: [30.0, 20.0, 25.0],
        up: [0.0, 0.0, 1.0],
    };
    let read = meshes(
        &model,
        &Meshes {
            view: None,
            camera: Some(camera),
            width: 1000,
            height: 1000,
            features: None,
            held: vec![],
        },
        &MeshesOn {
            kernel: &pool,
            meshes: &cache,
        },
    )
    .unwrap();
    assert_eq!(read.bodies.len(), 1);
    let body = &read.bodies[0];
    assert_eq!(body.body, Some(1));
    assert_eq!((body.level, body.deflection), (0.03125, 0.03125));
    assert_eq!(body.serves, [0.03125, 0.0625]);
    assert_eq!(body.bounds, Some([[0.0, 0.0, 0.0], [60.0, 40.0, 50.0]]));
    let mesh = body.mesh.as_ref().unwrap();
    // Six faces of two triangles each, named Face1 to Face6 in the mesh's order, stable.
    let faces: Vec<(String, bool, u32)> = body
        .faces
        .iter()
        .map(|f| (f.name.to_string(), f.stable, f.count.unwrap()))
        .collect();
    let want: Vec<(String, bool, u32)> = (1..=6)
        .map(|i| (format!("b0000001.box.Face{i}"), true, 2))
        .collect();
    assert_eq!(faces, want);
    assert_eq!(mesh.face_triangles, [2; 6]);
    assert_eq!(mesh.triangles.len(), 12);
    // Each face's triangles on its plane, their normals out of the box along its axis.
    let planes: [(usize, f64, [f32; 3]); 6] = [
        (0, 0.0, [-1.0, 0.0, 0.0]),
        (0, 60.0, [1.0, 0.0, 0.0]),
        (1, 0.0, [0.0, -1.0, 0.0]),
        (1, 40.0, [0.0, 1.0, 0.0]),
        (2, 0.0, [0.0, 0.0, -1.0]),
        (2, 50.0, [0.0, 0.0, 1.0]),
    ];
    let mut first = 0;
    for (face, &(axis, at, normal)) in planes.iter().enumerate() {
        for triangle in &mesh.triangles[first..first + 2] {
            for &v in triangle {
                let v = v as usize;
                assert_eq!(mesh.positions[v][axis], at, "face {}", face + 1);
                assert_eq!(mesh.normals[v], normal, "face {}", face + 1);
            }
        }
        first += 2;
    }
    // Twelve straight edges, two points each, named Edge1 to Edge12, stable.
    let edges: Vec<(String, bool, u32)> = body
        .edges
        .iter()
        .map(|e| (e.name.to_string(), e.stable, e.count.unwrap()))
        .collect();
    let want: Vec<(String, bool, u32)> = (1..=12)
        .map(|i| (format!("b0000001.box.Edge{i}"), true, 2))
        .collect();
    assert_eq!(edges, want);
    assert_eq!(mesh.edge_points.len(), 24);
    // Eight vertices, named Vertex1 to Vertex8, stable, with no count; their points the
    // box's corners exactly, in FreeCAD's (OCCT's) box order: x low then high, y low then
    // high within it, z high then low within that (a Part box's Vertex1 is (0, 0, H),
    // Vertex2 the origin).
    let vertices: Vec<(String, bool, Option<u32>)> = body
        .vertices
        .iter()
        .map(|v| (v.name.to_string(), v.stable, v.count))
        .collect();
    let want: Vec<(String, bool, Option<u32>)> = (1..=8)
        .map(|i| (format!("b0000001.box.Vertex{i}"), true, None))
        .collect();
    assert_eq!(vertices, want);
    assert_eq!(
        mesh.vertex_points,
        [
            [0.0, 0.0, 50.0],
            [0.0, 0.0, 0.0],
            [0.0, 40.0, 50.0],
            [0.0, 40.0, 0.0],
            [60.0, 0.0, 50.0],
            [60.0, 0.0, 0.0],
            [60.0, 40.0, 50.0],
            [60.0, 40.0, 0.0],
        ]
    );
    // Each an end of the edges the map says meet there: Vertex1 (0, 0, 50) ends Edge1, the
    // edge along z at x = 0, y = 0 (from (0, 0, 50) down to the origin, FreeCAD's Edge1).
    let edge1 = mesh.edges().next().unwrap();
    assert!(edge1.contains(&mesh.vertex_points[0]) && edge1.contains(&mesh.vertex_points[1]));
}
