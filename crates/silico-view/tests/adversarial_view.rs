//! break-it: hostile inputs for silico-view's library, which logs any mesh to Rerun for tests
//! and scripts: entity paths no part would have, meshes no kernel would send, and values
//! Rerun's 32-bit floats can't hold. Logged to memory, so no viewer is needed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::ErrorCode;
use silico_core::kernel::{Mesh, ShapeSummary};
use silico_view::{log_mesh, log_summary};

fn triangle() -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        triangles: vec![[0, 1, 2]],
        face_triangles: vec![1],
        edge_points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        edge_point_counts: vec![2],
    }
}

fn recording() -> (rerun::RecordingStream, rerun::sink::MemorySinkStorage) {
    rerun::RecordingStreamBuilder::new("silico-view-adversarial")
        .memory()
        .unwrap()
}

#[test]
fn any_entity_path_is_logged_without_a_panic() {
    // Paths a script might pass: empty, only separators, parent references, spaces,
    // right-to-left text and emoji, a NUL, Rerun's own syntax characters, and 100 000
    // characters. Rerun parses them forgivingly; logging must not panic or fail.
    let long = "x".repeat(100_000);
    let paths = [
        "",
        "/",
        "//",
        "a//b",
        "..",
        "../../etc",
        "a b",
        "ünïcødé/עברית/🦀",
        "a\0b",
        "[0]",
        "#",
        "\\",
        "a/faces",
        long.as_str(),
    ];
    let (rec, storage) = recording();
    for path in paths {
        log_mesh(&rec, path, &triangle()).unwrap();
        log_summary(
            &rec,
            path,
            &ShapeSummary {
                volume: f64::NAN,
                area: f64::INFINITY,
                center: [0.0; 3],
                faces: 0,
                edges: 0,
                vertices: 0,
                valid: false,
            },
        )
        .unwrap();
    }
    rec.flush_blocking().unwrap();
    assert!(!storage.take().is_empty());
}

#[test]
fn an_empty_mesh_is_logged() {
    let (rec, _storage) = recording();
    let empty = Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles: vec![],
        edge_points: vec![],
        edge_point_counts: vec![],
    };
    log_mesh(&rec, "empty", &empty).unwrap();
}

#[test]
fn logging_to_a_disabled_recording_is_fine() {
    let rec = rerun::RecordingStream::disabled();
    log_mesh(&rec, "part", &triangle()).unwrap();
}

#[test]
fn a_mesh_that_doesnt_hold_together_is_refused() {
    // A mesh that fails Mesh::validate (a triangle naming a vertex that isn't there,
    // normals for only some vertices, normals that aren't numbers) is refused as invalid
    // input. Mesh's fields are public, so a script or test can build one; log_mesh logged
    // them, and the viewer showed the part wrong or not at all, until /break-it found it.
    let mut out_of_range = triangle();
    out_of_range.triangles[0] = [0, 1, 7];
    let mut few_normals = triangle();
    few_normals.normals.truncate(1);
    let mut nan_normal = triangle();
    nan_normal.normals[0] = [f32::NAN, 0.0, 0.0];
    for (what, mesh) in [
        ("a vertex past the end", out_of_range),
        ("one normal for three vertices", few_normals),
        ("a NaN normal", nan_normal),
    ] {
        assert!(mesh.validate().is_err(), "{what}");
        let (rec, _storage) = recording();
        let err = log_mesh(&rec, "bad", &mesh).err();
        assert_eq!(
            err.as_ref().map(|e| e.code()),
            Some(ErrorCode::InvalidInput),
            "{what}: {err:?}"
        );
    }
}

#[test]
fn coordinates_rerun_cant_hold_are_refused() {
    // A valid mesh (finite f64 coordinates) with a vertex at 1e39 is refused: Rerun's
    // positions are f32, whose largest is 3.4e38, and log_mesh logged it at infinity until
    // /break-it found it. A hand-made or hostile B-rep reaches such a mesh (the kernel only
    // checks the extent is finite).
    let mut far_point = triangle();
    far_point.positions[1] = [1e39, 0.0, 0.0];
    let mut far_below = triangle();
    far_below.positions[1] = [-1e39, 0.0, 0.0];
    let mut far_edge = triangle();
    far_edge.edge_points[1] = [1e39, 0.0, 0.0];
    for (what, mesh) in [
        ("a vertex", far_point),
        ("a vertex below", far_below),
        ("an edge's point", far_edge),
    ] {
        assert_eq!(mesh.validate(), Ok(()), "{what}");
        let (rec, _storage) = recording();
        let err = log_mesh(&rec, "far", &mesh).err();
        assert_eq!(
            err.as_ref().map(|e| e.code()),
            Some(ErrorCode::InvalidInput),
            "{what}: {err:?}"
        );
    }
}

#[test]
fn the_summary_is_logged() {
    let (rec, storage) = recording();
    rec.flush_blocking().unwrap();
    storage.take();
    let summary = ShapeSummary {
        volume: 6000.0,
        area: 2200.0,
        center: [5.0, 10.0, 15.0],
        faces: 6,
        edges: 12,
        vertices: 8,
        valid: true,
    };
    log_summary(&rec, "part", &summary).unwrap();
    rec.flush_blocking().unwrap();
    assert!(!storage.take().is_empty(), "nothing was logged");
}
