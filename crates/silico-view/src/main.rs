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
use silico_view::{log_mesh, log_summary, rerun_error};

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

/// Couldn't open the file or start the viewer the user named: their input, not a bug.
fn sink_error(what: String, e: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::InvalidInput, what).with_cause(e.to_string())
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
    log_mesh(rec, part.name, mesh)?;
    log_summary(rec, part.name, summary)
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
    use silico_core::kernel::Primitive;

    use super::PARTS;

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
