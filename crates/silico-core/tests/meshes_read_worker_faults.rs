//! A worker's mesh that isn't its solid's, from the tests review of #244: refused as the
//! worker's (`protocol_error`) and dropped from the shared cache on every path, so the
//! next read on a well kernel asks again; at the body's floor too, and for a solid never
//! measured. And an eye inside a body not asked for is refused, as render refuses it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::example::EXAMPLE;
use silico_core::command::meshes::{Meshes, MeshesOn, MeshesRead, meshes};
use silico_core::command::{self, Problems};
use silico_core::document::FeatureId;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::Op;
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
use silico_core::recompute::{Key, Model, Recompute};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Camera, Mesh, MeshAt, Projection, Reply};

struct Bench {
    kernel: Arc<FakeKernel>,
    model: Arc<Model>,
    meshes: MeshCache<Key>,
}

impl Bench {
    fn with(text: &str, configure: impl FnOnce(&FakeKernel)) -> Bench {
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        configure(&kernel);
        let engine = Recompute::new(Arc::clone(&kernel)).with_sketches(FakeSketches::new());
        let model = command::rebuild(&engine, command::gate(text.as_bytes()).unwrap())
            .unwrap()
            .unwrap();
        Bench {
            kernel,
            model,
            meshes: MeshCache::new(MESH_CACHE_BYTES),
        }
    }
    fn read(&self, request: &Meshes) -> Result<MeshesRead, Problems> {
        meshes(
            &self.model,
            request,
            &MeshesOn {
                kernel: &self.kernel,
                meshes: &self.meshes,
            },
        )
    }
    fn meshed(&self) -> Vec<f64> {
        self.kernel
            .calls()
            .iter()
            .filter_map(|c| match &c.op {
                Op::Tessellate { deflection, .. } => Some(*deflection),
                _ => None,
            })
            .collect()
    }
}

fn front(view_height: f64) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

fn of(camera: Camera, width: u32, height: u32) -> Meshes {
    Meshes {
        view: None,
        camera: Some(camera),
        width,
        height,
        features: None,
        held: vec![],
    }
}

/// A mesh of `faces` faces and `edges` edges, and the example cut's ten vertices.
fn faces_and_edges(faces: usize, edges: usize) -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        normals: vec![[0.0, -1.0, 0.0]; 3],
        triangles: vec![[0, 1, 2]; faces],
        face_triangles: vec![1; faces],
        edge_points: vec![[0.0, 0.0, 0.0]; 2 * edges],
        edge_point_counts: vec![2; edges],
        vertex_points: vec![[0.0; 3]; 10],
        valid: None,
    }
}

fn boxes(boxes: &[(&str, [f64; 3], [f64; 3])]) -> String {
    let features: Vec<String> = boxes
        .iter()
        .map(|(id, [x, y, z], [l, w, h])| {
            format!(
                r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [{x}, {y}, {z}], "length": {l}, "width": {w}, "height": {h}}}"#
            )
        })
        .collect();
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 5, \"features\": [{}]}}",
        features.join(", ")
    )
}

/// a worker's bad mesh made at the body's floor (the kernel raised the deflection) is
/// forgotten too: the next read, the kernel well, asks again and answers 9 faces.
#[test]
fn a_bad_mesh_at_the_floor_is_forgotten() {
    let floor = 2144f64.sqrt() * 1e-4;
    let wrong = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&wrong);
    let bench = Bench::with(EXAMPLE, move |k| {
        k.answer_instead(move |op| match op {
            Op::Tessellate { deflection, .. } if flag.load(Ordering::SeqCst) => {
                Some(Reply::MeshAt(MeshAt {
                    mesh: faces_and_edges(10, 15),
                    deflection: deflection.max(floor),
                }))
            }
            _ => None,
        });
    });
    // 1 mm over 1,000 pixels: level 2^-11, under the floor.
    bench.read(&of(front(1.0), 1000, 1000)).unwrap_err();
    wrong.store(false, Ordering::SeqCst);
    let again = bench
        .read(&of(front(1.0), 1000, 1000))
        .unwrap_or_else(|p| panic!("the kernel well, still refused: {p:?}"));
    assert_eq!(again.bodies[0].faces.len(), 9);
    assert_eq!(bench.meshed().len(), 2, "{:?}", bench.meshed());
}

/// a solid never measured (its summary refused), meshed by a worker with fewer and
/// with more faces than it has, is the worker's fault, a protocol error, either way.
#[test]
fn an_unmeasured_solids_bad_mesh_is_the_workers_fault() {
    for faces in [3usize, 10] {
        let bench = Bench::with(EXAMPLE, move |k| {
            k.refuse(|op| {
                matches!(op, Op::Summarize { .. }).then(|| {
                    Error::new(ErrorCode::KernelFailed, "the measure failed")
                        .with_suggestion("measure again")
                })
            });
            k.answer_instead(move |op| match op {
                Op::Tessellate { deflection, .. } => Some(Reply::MeshAt(MeshAt {
                    mesh: faces_and_edges(faces, 15),
                    deflection: *deflection,
                })),
                _ => None,
            });
        });
        let result = bench.read(&of(front(80.0), 1000, 1000));
        let e = match result {
            Ok(read) => panic!(
                "{faces}: answered Ok with {} faces",
                read.bodies[0].faces.len()
            ),
            Err(p) => p.errors()[0].clone(),
        };
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{faces}: {e:?}");
    }
}

/// a perspective eye inside a body not asked for is refused too ("asked for or not").
#[test]
fn an_eye_inside_a_body_not_asked_for_is_refused() {
    let bench = Bench::with(
        &boxes(&[
            ("aaaaaaaa", [0.0; 3], [10.0; 3]),
            ("bbbbbbbb", [100.0, 0.0, 0.0], [10.0; 3]),
        ]),
        |_| {},
    );
    let inside_b = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [105.0, 5.0, 5.0],
        target: [0.0, 5.0, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    let result = bench.read(&Meshes {
        features: Some(vec![FeatureId::new("aaaaaaaa").unwrap()]),
        ..of(inside_b, 100, 100)
    });
    let e = result.map(|_| ()).unwrap_err().errors()[0].clone();
    assert_eq!(
        (e.parameter(), e.feature()),
        (Some("camera.eye"), Some("bbbbbbbb"))
    );
}

fn forging(summaries: bool, mesh: Mesh, wrong: Arc<AtomicBool>) -> Bench {
    Bench::with(EXAMPLE, move |k| {
        if !summaries {
            k.refuse(|op| {
                matches!(op, Op::Summarize { .. }).then(|| {
                    Error::new(ErrorCode::KernelFailed, "the measure failed")
                        .with_suggestion("measure again")
                })
            });
        }
        k.answer_instead(move |op| match op {
            Op::Tessellate { deflection, .. } if wrong.load(Ordering::SeqCst) => {
                Some(Reply::MeshAt(MeshAt {
                    mesh: mesh.clone(),
                    deflection: *deflection,
                }))
            }
            _ => None,
        });
    })
}

/// an unmeasured solid's bad mesh, refused, isn't kept: the next read, the kernel
/// well, recovers.
#[test]
fn an_unmeasured_solids_bad_mesh_is_forgotten() {
    let wrong = Arc::new(AtomicBool::new(true));
    let bench = forging(false, faces_and_edges(10, 15), Arc::clone(&wrong));
    bench.read(&of(front(80.0), 1000, 1000)).unwrap_err();
    wrong.store(false, Ordering::SeqCst);
    let again = bench
        .read(&of(front(80.0), 1000, 1000))
        .unwrap_or_else(|p| panic!("the kernel well, still refused: {p:?}"));
    assert_eq!(again.bodies[0].faces.len(), 9);
}

/// a measured solid's mesh with its faces right and its edges wrong is refused as the
/// worker's and forgotten.
#[test]
fn a_mesh_of_the_right_faces_and_wrong_edges_is_refused_and_forgotten() {
    let wrong = Arc::new(AtomicBool::new(true));
    let bench = forging(true, faces_and_edges(9, 16), Arc::clone(&wrong));
    let e = bench
        .read(&of(front(80.0), 1000, 1000))
        .map(|_| ())
        .unwrap_err()
        .errors()[0]
        .clone();
    assert_eq!(e.code(), ErrorCode::ProtocolError, "{e:?}");
    wrong.store(false, Ordering::SeqCst);
    let again = bench
        .read(&of(front(80.0), 1000, 1000))
        .unwrap_or_else(|p| panic!("the kernel well, still refused: {p:?}"));
    assert_eq!(again.bodies[0].edges.len(), 15);
}

/// A refused mesh's coarse deflection isn't kept as the step's floor: a faulty worker's
/// mesh of 10 faces for the cut's 9, said to be made at 0.5 for a view asking 2⁻⁵, is
/// refused, and the next read, the kernel well, meshes at 2⁻⁵, not at 0.5 (the architecture
/// review's low).
#[test]
fn a_refused_meshs_coarse_deflection_isnt_kept_as_the_floor() {
    let wrong = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&wrong);
    let bench = Bench::with(EXAMPLE, move |k| {
        k.answer_instead(move |op| match op {
            Op::Tessellate { .. } if flag.load(Ordering::SeqCst) => Some(Reply::MeshAt(MeshAt {
                mesh: faces_and_edges(10, 15),
                deflection: 0.5,
            })),
            _ => None,
        });
    });
    let e = bench
        .read(&of(front(80.0), 1000, 1000))
        .map(|_| ())
        .unwrap_err()
        .errors()[0]
        .clone();
    assert_eq!(e.code(), ErrorCode::ProtocolError, "{e:?}");
    wrong.store(false, Ordering::SeqCst);
    let again = bench
        .read(&of(front(80.0), 1000, 1000))
        .unwrap_or_else(|p| panic!("the kernel well, still refused: {p:?}"));
    assert_eq!(
        (again.bodies[0].level, again.bodies[0].deflection),
        (0.03125, 0.03125)
    );
}
