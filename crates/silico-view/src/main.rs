//! `silico-view`: shows what the kernel worker builds, in the Rerun viewer, until Silico
//! has its own app (ROADMAP.md). It runs the M0 operations through a real worker (a box,
//! a fillet, every edge rounded), meshes each result, and logs the faces, each edge's
//! polyline labelled with its number (`Edge1`, ...), and a summary of each part.
//!
//! Usage: `silico-view [--viewer <rerun executable>] [--save <file.rrd>]`. Without
//! `--save`, it starts the viewer. `pixi run view` runs it with the pinned viewer.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use silico_core::worker::Worker;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Generation, Mesh, Op, Reply, ShapeData, ShapeSummary};

const TIMEOUT: Duration = Duration::from_secs(60);

/// How far the mesh may stray from the surface, in millimetres.
const DEFLECTION: f64 = 0.02;

/// A part to show: a name, how it's made from a 10 × 20 × 30 box, and where it sits.
struct Part {
    name: &'static str,
    fillet: Option<(&'static [u32], f64)>,
    x: f32,
}

const PARTS: &[Part] = &[
    Part {
        name: "box",
        fillet: None,
        x: 0.0,
    },
    Part {
        name: "fillet_edge1_r2",
        fillet: Some((&[1], 2.0)),
        x: 25.0,
    },
    Part {
        name: "every_edge_r1",
        fillet: Some((&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 1.0)),
        x: 50.0,
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

/// The kernel worker: `SILICO_KERNEL_WORKER`, or the one built next to this executable.
fn worker_path() -> Result<PathBuf, Error> {
    if let Some(path) = std::env::var_os("SILICO_KERNEL_WORKER") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|e| {
        Error::new(ErrorCode::Internal, "can't find this executable").with_cause(e.to_string())
    })?;
    let worker = exe.with_file_name("silico-kernel-worker");
    if worker.exists() {
        Ok(worker)
    } else {
        Err(Error::new(
            ErrorCode::Internal,
            format!("no kernel worker at {}", worker.display()),
        )
        .with_suggestion("build it: cargo build -p silico-kernel --bin silico-kernel-worker"))
    }
}

fn shape_of(reply: Reply) -> Result<ShapeData, Error> {
    match reply {
        Reply::Shape(shape) => Ok(shape),
        other => Err(unexpected(&other)),
    }
}

fn unexpected(reply: &Reply) -> Error {
    Error::new(
        ErrorCode::ProtocolError,
        "the kernel worker sent an unexpected reply",
    )
    .with_cause(format!("{reply:?}"))
}

/// Builds `part` through the worker, and returns its mesh and summary.
fn build(worker: &mut Worker, part: &Part) -> Result<(Mesh, ShapeSummary), Error> {
    let mut shape = shape_of(worker.call(
        Generation(1),
        Op::MakeBox {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        },
        TIMEOUT,
    )?)?;
    if let Some((edges, radius)) = part.fillet {
        shape = shape_of(worker.call(
            Generation(1),
            Op::Fillet {
                shape,
                edges: edges.to_vec(),
                radius,
            },
            TIMEOUT,
        )?)?;
    }
    let summary = match worker.call(
        Generation(1),
        Op::Summarize {
            shape: shape.clone(),
        },
        TIMEOUT,
    )? {
        Reply::Summary(summary) => summary,
        other => return Err(unexpected(&other)),
    };
    let mesh = match worker.call(
        Generation(1),
        Op::Tessellate {
            shape,
            deflection: DEFLECTION,
        },
        TIMEOUT,
    )? {
        Reply::Mesh(mesh) => mesh,
        other => return Err(unexpected(&other)),
    };
    // Everything below indexes into the mesh: a worker's reply is checked first.
    mesh.validate().map_err(|problem| {
        Error::new(
            ErrorCode::ProtocolError,
            "the kernel worker sent an inconsistent mesh",
        )
        .with_cause(problem)
    })?;
    Ok((mesh, summary))
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
        &rerun::Transform3D::from_translation([part.x, 0.0, 0.0]),
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
    let mut worker = Worker::spawn(&worker_path()?)?;
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
        let (mesh, summary) = build(&mut worker, part)?;
        log_part(&rec, part, &mesh, &summary)?;
    }
    rec.flush_blocking().map_err(rerun_error)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use silico_protocol::Mesh;

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
        // Each part is 10 wide; they mustn't overlap.
        for pair in PARTS.windows(2) {
            assert!(
                pair[1].x - pair[0].x > 10.0,
                "{} and {}",
                pair[0].name,
                pair[1].name
            );
        }
    }
}
