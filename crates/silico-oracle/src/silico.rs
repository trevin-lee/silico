//! Silico's side: a case's shape built through the core's kernel API on a worker, and
//! measured.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use silico_core::kernel::{
    Axis, Blob, BooleanOp, ChamferSize, KernelWorker, Primitive, Profile, ShapeData,
};
use silico_core::{Error, ErrorCode};

use crate::case::{Chamfer, Shape};
use crate::compare::{Measured, Outcome};

/// What Silico builds and measures for `shape`: [`Outcome::Lost`] if the worker crashed,
/// hung, or answered out of turn, and was stopped (the next case needs a new one), or hit a
/// bug in Silico itself: neither may agree with a refusal.
pub fn measure(kernel: &mut KernelWorker, shape: &Shape) -> Outcome {
    let measured = build(kernel, shape).and_then(|s| {
        let summary = kernel.summarize(&s)?;
        Ok(Measured::new(&summary, kernel.measure_as_freecad(&s)?))
    });
    match measured {
        Ok(measured) => Outcome::Built(measured),
        Err(e) if lost(e.code()) => Outcome::Lost(e.to_string()),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// Whether an error with `code` means Silico couldn't finish, rather than refused.
pub(crate) fn lost(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::WorkerCrashed
            | ErrorCode::WorkerTimeout
            | ErrorCode::ProtocolError
            | ErrorCode::Internal
    )
}

/// `shape`, built through `kernel`.
pub fn build(kernel: &mut KernelWorker, shape: &Shape) -> Result<ShapeData, Error> {
    match shape {
        Shape::Brep { path } => converted(Path::new(path)),
        &Shape::Block { dx, dy, dz } => kernel.make(Primitive::Box { dx, dy, dz }),
        &Shape::Cylinder { radius, height } => kernel.make(Primitive::Cylinder { radius, height }),
        &Shape::Sphere { radius } => kernel.make(Primitive::Sphere { radius }),
        &Shape::Cone {
            radius1,
            radius2,
            height,
        } => kernel.make(Primitive::Cone {
            radius1,
            radius2,
            height,
        }),
        &Shape::Torus {
            major_radius,
            minor_radius,
        } => kernel.make(Primitive::Torus {
            major_radius,
            minor_radius,
        }),
        Shape::Translate { shape, by } => {
            let s = build(kernel, shape)?;
            Ok(kernel.translate(&s, *by)?.shape)
        }
        Shape::Fuse { base, tool } => boolean(kernel, BooleanOp::Fuse, base, tool),
        Shape::Cut { base, tool } => boolean(kernel, BooleanOp::Cut, base, tool),
        Shape::Common { base, tool } => boolean(kernel, BooleanOp::Common, base, tool),
        Shape::Fillet {
            shape,
            edges,
            radius,
        } => {
            let s = build(kernel, shape)?;
            Ok(kernel.fillet(&s, edges, *radius)?.shape)
        }
        Shape::Chamfer {
            shape,
            edges,
            size,
            flip,
        } => {
            let s = build(kernel, shape)?;
            let size = match *size {
                Chamfer::Equal { distance } => ChamferSize::Equal { distance },
                Chamfer::TwoDistances { distance, other } => {
                    ChamferSize::TwoDistances { distance, other }
                }
                Chamfer::DistanceAngle { distance, angle } => {
                    ChamferSize::DistanceAngle { distance, angle }
                }
            };
            Ok(kernel.chamfer(&s, edges, size, *flip)?.shape)
        }
        Shape::Extrude {
            profile,
            length,
            direction,
        } => {
            let profile = Profile {
                points: profile.clone(),
            };
            Ok(kernel.extrude(&profile, *length, *direction)?.shape)
        }
        Shape::Revolve {
            profile,
            origin,
            direction,
            angle,
        } => {
            let profile = Profile {
                points: profile.clone(),
            };
            let axis = Axis {
                origin: *origin,
                direction: *direction,
            };
            Ok(kernel.revolve(&profile, axis, *angle)?.shape)
        }
    }
}

fn boolean(
    kernel: &mut KernelWorker,
    op: BooleanOp,
    base: &Shape,
    tool: &Shape,
) -> Result<ShapeData, Error> {
    let base = build(kernel, base)?;
    let tool = build(kernel, tool)?;
    Ok(kernel.boolean(op, &base, &tool)?.shape)
}

/// Names a `silico-brep-convert` to use instead of the one beside this executable.
pub const BREP_CONVERT_ENV: &str = "SILICO_BREP_CONVERT";

/// How long `silico-brep-convert` may take over one part.
const CONVERT_DEADLINE: Duration = Duration::from_secs(60);

/// The text B-rep at `path` as Silico's B-rep, through `silico-brep-convert` (from the
/// kernel crate, the only one that reads B-reps): beside this executable (or its `deps/`
/// directory's parent, for tests), or where [`BREP_CONVERT_ENV`] says.
fn converted(path: &Path) -> Result<ShapeData, Error> {
    let tool = match std::env::var_os(BREP_CONVERT_ENV) {
        Some(tool) => PathBuf::from(tool),
        None => {
            let exe = std::env::current_exe().map_err(|e| {
                Error::new(ErrorCode::Internal, "can't find the running executable")
                    .with_cause(e.to_string())
            })?;
            let dir = exe.parent().unwrap_or(Path::new("."));
            let dir = if dir.file_name().is_some_and(|n| n == "deps") {
                dir.parent().unwrap_or(dir)
            } else {
                dir
            };
            dir.join(format!(
                "silico-brep-convert{}",
                std::env::consts::EXE_SUFFIX
            ))
        }
    };
    let text = std::fs::read(path).map_err(|e| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("can't read {}", path.display()),
        )
        .with_cause(e.to_string())
    })?;
    let mut child = Command::new(&tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            Error::new(
                ErrorCode::Internal,
                format!(
                    "can't run {} (cargo build -p silico-kernel --bins)",
                    tool.display()
                ),
            )
            .with_cause(e.to_string())
        })?;
    let fed = child
        .stdin
        .take()
        .map(|mut stdin| std::thread::spawn(move || stdin.write_all(&text)));
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as _));
    // OCCT's text reader crashes, or never returns, on some corrupt text.
    let deadline = Instant::now() + CONVERT_DEADLINE;
    let status = loop {
        let waited = child.try_wait().map_err(|e| {
            Error::new(ErrorCode::Internal, "can't wait for silico-brep-convert")
                .with_cause(e.to_string())
        })?;
        if let Some(status) = waited {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if let Some(fed) = fed {
        let _ = fed.join();
    }
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
    let refused = |why: String| {
        Err(Error::new(
            ErrorCode::InvalidInput,
            format!("{} can't be read", path.display()),
        )
        .with_cause(why))
    };
    match status {
        None => refused(format!(
            "silico-brep-convert took longer than {} s",
            CONVERT_DEADLINE.as_secs()
        )),
        Some(status) if !status.success() && stderr.is_empty() => {
            refused(format!("silico-brep-convert crashed ({status})"))
        }
        Some(status) if !status.success() => refused(stderr),
        Some(_) => Ok(ShapeData::unchecked(Blob::inline(stdout))),
    }
}

#[cfg(test)]
mod tests {
    use silico_core::ErrorCode;

    use super::lost;

    #[test]
    fn only_a_crash_a_hang_a_bad_reply_or_a_bug_is_lost() {
        let lost_codes: Vec<ErrorCode> = ErrorCode::ALL
            .iter()
            .copied()
            .filter(|&c| lost(c))
            .collect();
        assert_eq!(
            lost_codes,
            [
                ErrorCode::WorkerCrashed,
                ErrorCode::WorkerTimeout,
                ErrorCode::ProtocolError,
                ErrorCode::Internal,
            ]
        );
    }
}
