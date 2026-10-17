//! The sketch worker (ARCHITECTURE.md, Process model): the worker loop every worker shares
//! (`silico_worker`), solving sketches with PlaneGCS. It keeps no state: each request
//! carries the whole sketch.

use silico_protocol::{Failure, Op, Refusal, Reply};

/// How the sketch worker reads and switches off floating-point traps: directly, as it
/// links no library that manages them.
pub const PLATFORM: silico_worker::Platform = silico_worker::Platform {
    fp_trap_mask: crate::fp_trap_mask,
    disable_fp_traps: crate::disable_fp_traps,
    built_from: "PlaneGCS from FreeCAD 1.1.3",
};

/// The production operations: a solve, and the echo every worker answers. Anything else
/// is another worker's (or a test operation).
pub fn handle(op: Op) -> Result<Reply, Failure> {
    match op {
        Op::Solve(system) => {
            // The core checked it before sending; checked again, since PlaneGCS trusts its
            // input entirely.
            system.check().map_err(|e| {
                Failure::new(Refusal::InvalidParameter {
                    parameter: e.parameter.to_owned(),
                    message: e.message,
                })
            })?;
            Ok(Reply::Solved(crate::solve(&system)?))
        }
        Op::Echo { data } => Ok(Reply::Echo { data }),
        Op::Debug(_) => Err(Failure::new(Refusal::TestOperation {
            op: op.name().to_owned(),
        })),
        other => Err(Failure::new(Refusal::OtherWorkersOp {
            op: other.name().to_owned(),
        })),
    }
}
