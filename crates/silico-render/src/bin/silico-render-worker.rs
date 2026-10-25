//! The render worker the core starts (ARCHITECTURE.md, Process model). It reads its adapter
//! policy from its arguments (`RenderAdapter::from_args`: `--any-adapter` lets it draw on a
//! GPU when there's no lavapipe; by default it draws only on lavapipe), restarts itself with
//! lavapipe's variables first (`silico_render::lavapipe`), then serves its inherited socket.
//! Exits 2 for an argument it doesn't know, and 4 if it can't restart, saying why.

use std::io::Write;
use std::process::ExitCode;

use silico_render::worker::{PLATFORM, RenderAdapter, State};

fn main() -> ExitCode {
    let policy = match RenderAdapter::from_args(std::env::args_os().skip(1)) {
        Ok(policy) => policy,
        Err(problem) => {
            let _ = writeln!(std::io::stderr(), "silico-render-worker: {problem}");
            return ExitCode::from(2);
        }
    };
    if let Err(problem) = silico_render::lavapipe::reexec_with() {
        let _ = writeln!(std::io::stderr(), "silico-render-worker: {problem}");
        return ExitCode::from(4);
    }
    let mut state = State::new(policy);
    silico_worker::run(&PLATFORM, move |_, _, op, _| state.handle(op))
}
