//! Logs what the kernel builds to Rerun, for tests and scripts as well as the `silico-view`
//! binary: a shape's mesh (its faces, shaded by the surface normals the kernel gives, and
//! each edge's polyline labelled with its number, `Edge1`, ...), and a summary of it.

use silico_core::kernel::{Mesh, ShapeSummary};
use silico_core::{Error, ErrorCode};

/// Logs `mesh` under `path`: its faces at `path/faces`, its edges at `path/edges`. A mesh
/// that doesn't hold together ([`Mesh::validate`]), or that has a coordinate past what
/// Rerun's 32-bit floats hold, is refused.
pub fn log_mesh(rec: &rerun::RecordingStream, path: &str, mesh: &Mesh) -> Result<(), Error> {
    mesh.validate().map_err(|e| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("the mesh is unusable: {e}"),
        )
    })?;
    let largest = f64::from(f32::MAX);
    if let Some(p) = mesh
        .positions
        .iter()
        .chain(&mesh.edge_points)
        .find(|p| p.iter().any(|c| c.abs() > largest))
    {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the mesh's point {p:?} is too far out for Rerun, which holds coordinates as 32-bit floats"
            ),
        ));
    }
    rec.log_static(format!("{path}/faces"), &faces(mesh))
        .map_err(rerun_error)?;
    rec.log_static(format!("{path}/edges"), &edges(mesh))
        .map_err(rerun_error)
}

/// Logs `summary` under `path/summary`, as text.
pub fn log_summary(
    rec: &rerun::RecordingStream,
    path: &str,
    summary: &ShapeSummary,
) -> Result<(), Error> {
    rec.log_static(
        format!("{path}/summary"),
        &rerun::TextDocument::new(summary_text(summary)),
    )
    .map_err(rerun_error)
}

fn summary_text(summary: &ShapeSummary) -> String {
    format!(
        "volume {:.6}\narea {:.6}\nfaces {}, edges {}, vertices {}\nvalid: {}",
        summary.volume, summary.area, summary.faces, summary.edges, summary.vertices, summary.valid
    )
}

/// A failure to log, or to flush what was logged.
pub fn rerun_error(e: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::Internal, "couldn't log to Rerun").with_cause(e.to_string())
}

/// The mesh's faces, each vertex with its normal from the kernel (vertices aren't shared
/// between faces, so edges between them stay sharp).
fn faces(mesh: &Mesh) -> rerun::Mesh3D {
    rerun::Mesh3D::new(mesh.positions.iter().copied().map(point))
        .with_triangle_indices(mesh.triangles.iter().copied())
        .with_vertex_normals(mesh.normals.iter().copied())
        .with_albedo_factor(rerun::Rgba32::from_rgb(180, 190, 205))
}

/// The mesh's edges, one polyline each that has points, labelled with its number.
fn edges(mesh: &Mesh) -> rerun::LineStrips3D {
    let (strips, labels) = edge_strips(mesh);
    rerun::LineStrips3D::new(strips)
        .with_labels(labels)
        .with_colors([rerun::Color::from_rgb(30, 30, 30)])
}

fn point(p: [f64; 3]) -> [f32; 3] {
    p.map(|c| c as f32)
}

/// One polyline per edge that has points, labelled with its number: the first is Edge1.
fn edge_strips(mesh: &Mesh) -> (Vec<Vec<[f32; 3]>>, Vec<String>) {
    mesh.edges()
        .enumerate()
        .filter(|(_, points)| !points.is_empty())
        .map(|(i, points)| {
            (
                points.iter().copied().map(point).collect(),
                format!("Edge{}", i + 1),
            )
        })
        .unzip()
}

#[cfg(test)]
mod tests {
    use silico_core::kernel::{Mesh, ShapeSummary};

    use super::{edge_strips, edges, faces, summary_text};

    fn triangle() -> Mesh {
        Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0], [0.6, 0.0, 0.8], [0.0, 0.6, 0.8]],
            triangles: vec![[0, 1, 2]],
            face_triangles: vec![1],
            edge_points: vec![
                [0.0, 0.0, 30.0],
                [0.0, 0.0, 0.0],
                [1.0, 2.0, 3.0],
                [4.0, 5.0, 6.0],
                [7.0, 8.0, 9.0],
            ],
            edge_point_counts: vec![2, 0, 3],
        }
    }

    #[test]
    fn edges_are_labelled_by_their_number_and_degenerate_ones_skipped() {
        let (strips, labels) = edge_strips(&triangle());
        assert_eq!(labels, ["Edge1", "Edge3"]);
        assert_eq!(strips[0], [[0.0, 0.0, 30.0], [0.0, 0.0, 0.0]]);
        assert_eq!(strips[1].len(), 3);
    }

    #[test]
    fn the_summary_says_each_measure() {
        let summary = ShapeSummary {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, 15.0],
            faces: 6,
            edges: 12,
            vertices: 8,
            valid: true,
        };
        assert_eq!(
            summary_text(&summary),
            "volume 6000.000000\narea 2200.000000\nfaces 6, edges 12, vertices 8\nvalid: true"
        );
    }

    #[test]
    fn edges_are_logged_with_their_labels() {
        use rerun::external::arrow::array::{Array, StringArray};
        let logged = edges(&triangle());
        let labels = logged.labels.unwrap().array;
        let labels = labels.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(
            labels.iter().collect::<Vec<_>>(),
            [Some("Edge1"), Some("Edge3")]
        );
        assert_eq!(logged.strips.unwrap().array.len(), 2);
    }

    #[test]
    fn faces_are_shaded_by_the_kernels_normals() {
        // Not normals estimated from the triangles: those would all be +z here.
        use rerun::external::arrow::array::{Array, FixedSizeListArray, Float32Array};
        let logged = faces(&triangle()).vertex_normals.unwrap().array;
        let values = logged
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .unwrap()
            .values()
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap()
            .values()
            .chunks_exact(3)
            .map(|n| [n[0], n[1], n[2]])
            .collect::<Vec<_>>();
        assert_eq!(values, triangle().normals);
    }
}
