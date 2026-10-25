//! The render worker with the test operations, for tests of the core's render client:
//! sleeping, aborting, garbage, and replies out of turn. The client sends nothing but
//! renders, so two scene sizes no render needs ask for its misbehaviour too: a scene 1 by 7
//! hangs (a render past its time), and one 1 by 9 aborts (a worker that crashes). Built only
//! with the `test_ops` feature, and kept in its own binary so the production worker's code
//! never dispatches these.

use std::io::Write;
use std::process::ExitCode;

use silico_protocol::{Failure, Op, Refusal};
use silico_render::worker::{PLATFORM, RenderAdapter, State};

fn main() -> ExitCode {
    if let Err(problem) = silico_render::lavapipe::reexec_with() {
        let _ = writeln!(std::io::stderr(), "silico-render-test-worker: {problem}");
        return ExitCode::from(4);
    }
    let mut state = State::new(RenderAdapter::Software);
    silico_worker::run(&PLATFORM, move |id, generation, op, socket| match op {
        Op::Debug(debug) => match silico_worker::test_ops::answer(id, generation, debug, socket) {
            Ok(answered) => answered,
            Err(other) => Err(Failure::new(Refusal::TestOperation {
                op: other.name().to_owned(),
            })),
        },
        Op::Render(scene) if (scene.width, scene.height) == (1, 7) => {
            std::thread::sleep(std::time::Duration::from_secs(600));
            state.handle(Op::Render(scene))
        }
        Op::Render(scene) if (scene.width, scene.height) == (1, 9) => std::process::abort(),
        op => state.handle(op),
    })
}
