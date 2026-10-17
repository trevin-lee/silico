//! The test operations every worker's test binary answers the same way, for tests of how
//! the core treats a worker that sleeps, aborts, writes garbage, or replies out of turn.
//! Crashing with a fault and switching traps on go through the C++ a worker links, so
//! each test worker answers those itself.

use std::io::Write;

use silico_protocol::{
    DebugOp, Failure, Generation, OutOfTurn, Refusal, Reply, RequestId, Response, write_message,
};

/// Answers `op` if it's one of the shared test operations; otherwise gives it back.
pub fn answer(
    id: RequestId,
    generation: Generation,
    op: DebugOp,
    socket: &mut dyn Write,
) -> Result<Result<Reply, Failure>, DebugOp> {
    Ok(match op {
        DebugOp::Sleep { millis } => {
            std::thread::sleep(std::time::Duration::from_millis(millis));
            Ok(Reply::Done)
        }
        DebugOp::Abort => {
            // More lines than a crash report keeps, so the client's cap shows.
            for line in 1..=25 {
                tracing::warn!("aborting, as asked: line {line}");
            }
            std::process::abort()
        }
        DebugOp::Garbage => {
            // A header claiming the largest possible frame: never a valid reply.
            socket
                .write_all(&[0xff; 16])
                .and_then(|()| socket.flush())
                .map_err(|e| internal(e.to_string()))
                .map(|()| Reply::Done)
        }
        DebugOp::ReplyOutOfTurn(out_of_turn) => {
            let stale = Response {
                id: match out_of_turn {
                    OutOfTurn::WrongId => RequestId(id.0.wrapping_sub(1)),
                    OutOfTurn::WrongGeneration => id,
                },
                generation: match out_of_turn {
                    OutOfTurn::WrongId => generation,
                    OutOfTurn::WrongGeneration => Generation(generation.0.wrapping_add(1)),
                },
                result: Ok(Reply::Done),
            };
            write_message(socket, &stale)
                .map_err(|e| internal(e.to_string()))
                .map(|()| Reply::Done)
        }
        other => return Err(other),
    })
}

fn internal(what: String) -> Failure {
    Failure::new(Refusal::Internal { what })
}
