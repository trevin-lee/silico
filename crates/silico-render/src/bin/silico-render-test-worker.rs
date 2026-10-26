//! The render worker with the test operations, for tests of the core's render client:
//! sleeping, aborting, garbage, and replies out of turn, and for renders, the misbehaviour
//! each of `silico_render::test_scenes`'s sizes asks for. It reads its arguments as the
//! worker does. Built only with the `test_ops` feature, and kept in its own binary so the
//! production worker's code never dispatches these.

use std::io::Write;
use std::process::ExitCode;

use silico_protocol::{Blob, Failure, Op, Refusal, Reply};
use silico_render::test_scenes::{
    CRASHES, HANGS, IDS_FLIPPED, IDS_SHORT, NO_IMAGE, REFUSES, SAYS_POLICY, WRONG_REPLY,
};
use silico_render::worker::{PLATFORM, RenderAdapter, State};

fn main() -> ExitCode {
    let policy = match RenderAdapter::from_args(std::env::args_os().skip(1)) {
        Ok(policy) => policy,
        Err(problem) => {
            let _ = writeln!(std::io::stderr(), "silico-render-test-worker: {problem}");
            return ExitCode::from(2);
        }
    };
    if let Err(problem) = silico_render::lavapipe::reexec_with() {
        let _ = writeln!(std::io::stderr(), "silico-render-test-worker: {problem}");
        return ExitCode::from(4);
    }
    let mut state = State::new(policy);
    silico_worker::run(&PLATFORM, move |id, generation, op, socket| match op {
        Op::Debug(debug) => match silico_worker::test_ops::answer(id, generation, debug, socket) {
            Ok(answered) => answered,
            Err(other) => Err(Failure::new(Refusal::TestOperation {
                op: other.name().to_owned(),
            })),
        },
        Op::Render(scene) => match (scene.width, scene.height) {
            HANGS => {
                std::thread::sleep(std::time::Duration::from_secs(600));
                state.handle(Op::Render(scene))
            }
            CRASHES => std::process::abort(),
            IDS_SHORT => match state.handle(Op::Render(scene))? {
                Reply::Rendered { png, ids } => Ok(Reply::Rendered {
                    png,
                    ids: ids.map(|mut ids| {
                        ids.pop();
                        ids
                    }),
                }),
                other => Ok(other),
            },
            NO_IMAGE => Ok(Reply::Rendered {
                png: Vec::new(),
                ids: scene.ids.then(|| vec![[0, 0]; 21]),
            }),
            WRONG_REPLY => Ok(Reply::Echo {
                data: Blob::inline(Vec::new()),
            }),
            IDS_FLIPPED => match state.handle(Op::Render(scene))? {
                Reply::Rendered { png, ids } => Ok(Reply::Rendered {
                    png,
                    ids: match ids {
                        Some(_) => None,
                        None => Some(Vec::new()),
                    },
                }),
                other => Ok(other),
            },
            REFUSES => Err(Failure::new(Refusal::InvalidParameter {
                parameter: "width".to_owned(),
                message: "the test worker refuses this scene".to_owned(),
            })),
            SAYS_POLICY => Err(Failure::new(Refusal::NoAdapter {
                cause: format!("policy {policy:?}"),
            })),
            _ => state.handle(Op::Render(scene)),
        },
        op => state.handle(op),
    })
}
