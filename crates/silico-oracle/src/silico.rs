//! Silico's side: a case's shape built through the core's kernel API on a worker, and
//! measured.

use silico_core::kernel::{
    Axis, BooleanOp, ChamferSize, KernelWorker, Primitive, Profile, ShapeData,
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
fn lost(code: ErrorCode) -> bool {
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
