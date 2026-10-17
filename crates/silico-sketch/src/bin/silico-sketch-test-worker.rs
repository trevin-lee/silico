//! The sketch worker with the test operations, for tests of how the core treats it:
//! sleeping, aborting, crashing, replying with garbage or out of turn, and a C++ exception.
//! Built only with the `test_ops` feature, and kept in its own binary so the production
//! worker's code never dispatches these.

use silico_protocol::{DebugOp, Failure, Op, Refusal, Reply};

fn main() -> std::process::ExitCode {
    silico_worker::run(
        &silico_sketch::worker::PLATFORM,
        |id, generation, op, socket| match op {
            Op::Debug(op) => match silico_worker::test_ops::answer(id, generation, op, socket) {
                Ok(result) => result,
                Err(DebugOp::Segfault) => {
                    silico_sketch::test_ops::raise_sigsegv();
                    Ok(Reply::Done)
                }
                Err(other) => Err(Failure::new(Refusal::TestOperation {
                    op: Op::Debug(other).name().to_owned(),
                })),
            },
            other => silico_sketch::worker::handle(other),
        },
    )
}
