//! `silico-view`: shows what the kernel worker builds, in the Rerun viewer, until Silico
//! has its own app (ROADMAP.md). It makes each part through the core's kernel API (a box
//! and its fillets, then the other primitives), meshes it, and logs the faces, each edge's
//! polyline labelled with its number (`Edge1`, ...), and a summary of each part.
//!
//! Usage: `silico-view [--viewer <rerun executable>] [--save <file.rrd>]`. Without
//! `--save`, it starts the viewer. `pixi run view` runs it with the pinned viewer.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use silico_core::kernel::{KernelWorker, Mesh, Primitive, ShapeSummary};
use silico_core::{Error, ErrorCode};

/// How far the mesh may stray from the surface, in millimetres.
const DEFLECTION: f64 = 0.02;

/// A part to show: a name, how it's made, and where it sits. Parts are laid out in rows,
/// 30 apart, the rows 40 apart; none is more than 24 across.
struct Part {
    name: &'static str,
    primitive: Primitive,
    fillet: Option<(&'static [u32], f64)>,
    at: [f32; 2],
}

const BOX: Primitive = Primitive::Box {
    dx: 10.0,
    dy: 20.0,
    dz: 30.0,
};

const PARTS: &[Part] = &[
    Part {
        name: "box",
        primitive: BOX,
        fillet: None,
        at: [0.0, 0.0],
    },
    Part {
        name: "fillet_edge1_r2",
        primitive: BOX,
        fillet: Some((&[1], 2.0)),
        at: [30.0, 0.0],
    },
    Part {
        name: "every_edge_r1",
        primitive: BOX,
        fillet: Some((&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 1.0)),
        at: [60.0, 0.0],
    },
    Part {
        name: "cylinder",
        primitive: Primitive::Cylinder {
            radius: 5.0,
            height: 10.0,
        },
        fillet: None,
        at: [0.0, 40.0],
    },
    Part {
        name: "sphere",
        primitive: Primitive::Sphere { radius: 5.0 },
        fillet: None,
        at: [30.0, 40.0],
    },
    Part {
        name: "cone",
        primitive: Primitive::Cone {
            radius1: 5.0,
            radius2: 2.0,
            height: 10.0,
        },
        fillet: None,
        at: [60.0, 40.0],
    },
    Part {
        name: "torus",
        primitive: Primitive::Torus {
            major_radius: 10.0,
            minor_radius: 2.0,
        },
        fillet: None,
        at: [90.0, 40.0],
    },
];

/// What the command line asks for.
enum Command {
    Help,
    Show(Sink),
}

/// Where the parts go.
enum Sink {
    Viewer(Option<String>),
    File(PathBuf),
}

const USAGE: &str = "usage: silico-view [--viewer <rerun executable>] [--save <file.rrd>]\n\
                     Shows a box and two fillets, built by the kernel worker, in Rerun.";

fn main() -> ExitCode {
    // Logs (the kernel worker's among them) go to stderr, filtered by SILICO_LOG.
    silico_core::logging::init();
    let mut out = std::io::stdout().lock();
    let result = parse_args().and_then(|command| match command {
        Command::Help => Ok(USAGE.to_owned()),
        Command::Show(sink) => run(sink),
    });
    match result {
        Ok(message) => {
            let _ = writeln!(out, "{message}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr().lock(), "silico-view: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Command, Error> {
    let mut args = std::env::args().skip(1);
    let mut sink = Sink::Viewer(None);
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(Command::Help);
        }
        let value = args.next().ok_or_else(|| {
            Error::new(ErrorCode::InvalidInput, format!("{arg} needs a value"))
                .with_suggestion(USAGE)
        })?;
        sink = match arg.as_str() {
            "--viewer" => Sink::Viewer(Some(value)),
            "--save" => Sink::File(PathBuf::from(value)),
            _ => {
                return Err(
                    Error::new(ErrorCode::InvalidInput, format!("unknown argument {arg}"))
                        .with_suggestion(USAGE),
                );
            }
        };
    }
    Ok(Command::Show(sink))
}

/// Makes `part` through the kernel, and returns its mesh and summary.
fn build(kernel: &mut KernelWorker, part: &Part) -> Result<(Mesh, ShapeSummary), Error> {
    let mut shape = kernel.make(part.primitive)?;
    if let Some((edges, radius)) = part.fillet {
        shape = kernel.fillet(&shape, edges, radius)?.shape;
    }
    Ok((
        kernel.tessellate(&shape, DEFLECTION)?,
        kernel.summarize(&shape)?,
    ))
}

fn rerun_error(e: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::Internal, "couldn't log to Rerun").with_cause(e.to_string())
}

/// Couldn't open the file or start the viewer the user named: their input, not a bug.
fn sink_error(what: String, e: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::InvalidInput, what).with_cause(e.to_string())
}

fn point(p: [f64; 3]) -> [f32; 3] {
    p.map(|c| c as f32)
}

/// Per-vertex normals for smooth shading within each face (vertices aren't shared
/// between faces, so edges stay sharp).
fn normals(mesh: &Mesh) -> Vec<[f32; 3]> {
    let mut sums = vec![[0.0f64; 3]; mesh.positions.len()];
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        let (u, v) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &i in t {
            for k in 0..3 {
                sums[i as usize][k] += n[k];
            }
        }
    }
    sums.into_iter()
        .map(|n| {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len > 0.0 {
                point([n[0] / len, n[1] / len, n[2] / len])
            } else {
                [0.0, 0.0, 1.0]
            }
        })
        .collect()
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

fn log_part(
    rec: &rerun::RecordingStream,
    part: &Part,
    mesh: &Mesh,
    summary: &ShapeSummary,
) -> Result<(), Error> {
    rec.log_static(
        part.name,
        &rerun::Transform3D::from_translation([part.at[0], part.at[1], 0.0]),
    )
    .map_err(rerun_error)?;
    rec.log_static(
        format!("{}/faces", part.name),
        &rerun::Mesh3D::new(mesh.positions.iter().copied().map(point))
            .with_triangle_indices(mesh.triangles.iter().copied())
            .with_vertex_normals(normals(mesh))
            .with_albedo_factor(rerun::Rgba32::from_rgb(180, 190, 205)),
    )
    .map_err(rerun_error)?;
    let (strips, labels) = edge_strips(mesh);
    rec.log_static(
        format!("{}/edges", part.name),
        &rerun::LineStrips3D::new(strips)
            .with_labels(labels)
            .with_colors([rerun::Color::from_rgb(30, 30, 30)]),
    )
    .map_err(rerun_error)?;
    rec.log_static(
        format!("{}/summary", part.name),
        &rerun::TextDocument::new(format!(
            "volume {:.6}\narea {:.6}\nfaces {}, edges {}, vertices {}\nvalid: {}",
            summary.volume,
            summary.area,
            summary.faces,
            summary.edges,
            summary.vertices,
            summary.valid
        )),
    )
    .map_err(rerun_error)?;
    Ok(())
}

fn run(sink: Sink) -> Result<String, Error> {
    let mut kernel = KernelWorker::start()?;
    let builder = rerun::RecordingStreamBuilder::new("silico");
    let (rec, done) = match &sink {
        Sink::File(path) => (
            builder
                .save(path)
                .map_err(|e| sink_error(format!("couldn't write {}", path.display()), e))?,
            format!("saved to {}", path.display()),
        ),
        Sink::Viewer(executable) => {
            let opts = rerun::SpawnOptions {
                executable_path: executable.clone(),
                ..Default::default()
            };
            (
                builder.spawn_opts(&opts).map_err(|e| {
                    sink_error(
                        format!("couldn't start the Rerun viewer {}", opts.executable_path()),
                        e,
                    )
                })?,
                "sent to the Rerun viewer".to_owned(),
            )
        }
    };
    // Z up, as in CAD.
    rec.log_static("/", &rerun::ViewCoordinates::RIGHT_HAND_Z_UP())
        .map_err(rerun_error)?;
    for part in PARTS {
        let (mesh, summary) = build(&mut kernel, part)?;
        log_part(&rec, part, &mesh, &summary)?;
    }
    rec.flush_blocking().map_err(rerun_error)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use silico_core::kernel::{Mesh, Primitive};

    use super::{PARTS, edge_strips};

    #[test]
    fn edges_are_labelled_by_their_number_and_degenerate_ones_skipped() {
        let mesh = Mesh {
            positions: vec![],
            triangles: vec![],
            face_triangles: vec![],
            edge_points: vec![
                [0.0, 0.0, 30.0],
                [0.0, 0.0, 0.0],
                [1.0, 2.0, 3.0],
                [4.0, 5.0, 6.0],
                [7.0, 8.0, 9.0],
            ],
            edge_point_counts: vec![2, 0, 3],
        };
        let (strips, labels) = edge_strips(&mesh);
        assert_eq!(labels, ["Edge1", "Edge3"]);
        assert_eq!(strips[0], [[0.0, 0.0, 30.0], [0.0, 0.0, 0.0]]);
        assert_eq!(strips[1].len(), 3);
    }

    #[test]
    fn parts_sit_apart() {
        // Each part's footprint on the ground, from its primitive's sizes (a fillet only
        // removes material): no two overlap.
        let footprint = |part: &super::Part| -> [f32; 4] {
            let [x, y] = part.at;
            let round = |r: f64| [x - r as f32, y - r as f32, x + r as f32, y + r as f32];
            match part.primitive {
                Primitive::Box { dx, dy, .. } => [x, y, x + dx as f32, y + dy as f32],
                Primitive::Cylinder { radius, .. } | Primitive::Sphere { radius } => round(radius),
                Primitive::Cone {
                    radius1, radius2, ..
                } => round(radius1.max(radius2)),
                Primitive::Torus {
                    major_radius,
                    minor_radius,
                } => round(major_radius + minor_radius),
            }
        };
        for (i, a) in PARTS.iter().enumerate() {
            for b in &PARTS[i + 1..] {
                let ([a0, a1, a2, a3], [b0, b1, b2, b3]) = (footprint(a), footprint(b));
                let apart = a2 < b0 || b2 < a0 || a3 < b1 || b3 < a1;
                assert!(apart, "{} and {} overlap", a.name, b.name);
            }
        }
    }
}
