//! The kernel worker with the test operations, for tests of the core's worker client:
//! sleeping, aborting, crashing, replying with garbage, switching floating-point traps on,
//! and misbehaving on request. Built only with the `test_ops` feature, and kept in its own
//! binary so the production worker's code never dispatches these.

use silico_kernel::{test_ops, worker};
use silico_protocol::{
    DebugOp, Failure, FailureKind, Generation, Mesh, Misbehavior, Op, OutOfTurn, Reply, RequestId,
    Response, write_message,
};

fn main() -> std::process::ExitCode {
    let mut misbehavior = None;
    worker::run(move |id, generation, op, socket| match op {
        Op::Debug(DebugOp::Misbehave(how)) => {
            misbehavior = Some(how);
            Ok(Reply::Done)
        }
        Op::Debug(_) => debug(id, generation, op, socket),
        Op::Tessellate { .. } if misbehavior == Some(Misbehavior::BadMesh) => {
            Ok(Reply::Mesh(Mesh {
                positions: vec![[0.0; 3]; 3],
                triangles: vec![[0, 1, 3]],
                face_triangles: vec![1],
                edge_points: vec![],
                edge_point_counts: vec![],
            }))
        }
        _ if misbehavior == Some(Misbehavior::WrongReply) => Ok(Reply::Done),
        Op::Fillet { .. } | Op::Translate { .. } | Op::Boolean { .. }
            if matches!(
                misbehavior,
                Some(Misbehavior::BadHistory | Misbehavior::HistoryForNoInputs)
            ) =>
        {
            let mut reply = worker::handle(op)?;
            if let Reply::Built(built) = &mut reply {
                if misbehavior == Some(Misbehavior::HistoryForNoInputs) {
                    built.history.inputs.clear();
                } else if let Some(face) = built
                    .history
                    .inputs
                    .first_mut()
                    .and_then(|i| i.faces.first_mut())
                {
                    face.kept = Some(99);
                }
            }
            Ok(reply)
        }
        other => worker::handle(other),
    })
}

/// The test operations other than `Misbehave`.
fn debug(
    id: RequestId,
    generation: Generation,
    op: Op,
    socket: &mut dyn std::io::Write,
) -> Result<Reply, Failure> {
    match op {
        Op::Debug(DebugOp::Sleep { millis }) => {
            std::thread::sleep(std::time::Duration::from_millis(millis));
            Ok(Reply::Done)
        }
        Op::Debug(DebugOp::Abort) => {
            // More lines than a crash report keeps, so the client's cap shows.
            for line in 1..=25 {
                tracing::warn!("aborting, as asked: line {line}");
            }
            std::process::abort()
        }
        Op::Debug(DebugOp::Segfault) => {
            test_ops::raise_sigsegv();
            Ok(Reply::Done)
        }
        Op::Debug(DebugOp::Garbage) => {
            // A header claiming the largest possible frame: never a valid reply.
            socket
                .write_all(&[0xff; 16])
                .and_then(|()| socket.flush())
                .map_err(|e| Failure::new(FailureKind::Internal, e.to_string()))?;
            Ok(Reply::Done)
        }
        Op::Debug(DebugOp::EnableFpTraps) => {
            test_ops::enable_fp_traps()
                .map_err(|e| Failure::new(FailureKind::Internal, e.message().to_owned()))?;
            Ok(Reply::Done)
        }
        Op::Debug(DebugOp::ReplyOutOfTurn(out_of_turn)) => {
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
                .map_err(|e| Failure::new(FailureKind::Internal, e.to_string()))?;
            Ok(Reply::Done)
        }
        other => worker::handle(other),
    }
}
