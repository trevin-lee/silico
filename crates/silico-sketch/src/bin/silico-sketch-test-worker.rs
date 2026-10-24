//! The sketch worker with the test operations, for tests of how the core treats it:
//! sleeping, aborting, crashing, replying with garbage or out of turn, switching
//! floating-point traps on, and throwing `std::bad_alloc` through the bridge.
//! Built only with the `test_ops` feature, and kept in its own binary so the production
//! worker's code never dispatches these.

use silico_protocol::{DebugOp, Failure, Op, Refusal, Reply};

fn main() -> std::process::ExitCode {
    let mut state = silico_sketch::worker::State::default();
    silico_worker::run(
        &silico_sketch::worker::PLATFORM,
        move |id, generation, op, socket| match op {
            Op::Debug(op) => match silico_worker::test_ops::answer(id, generation, op, socket) {
                Ok(result) => result,
                Err(DebugOp::Segfault) => {
                    silico_sketch::test_ops::raise_sigsegv();
                    Ok(Reply::Done)
                }
                Err(DebugOp::EnableFpTraps) => silico_sketch::test_ops::enable_fp_traps()
                    .map(|()| Reply::Done)
                    .map_err(|what| Failure::new(Refusal::Internal { what })),
                Err(DebugOp::ThrowBadAlloc) => {
                    silico_sketch::test_ops::throw_bad_alloc().map(|()| Reply::Done)
                }
                Err(other) => Err(Failure::new(Refusal::TestOperation {
                    op: Op::Debug(other).name().to_owned(),
                })),
            },
            other => state.handle(other),
        },
    )
}
