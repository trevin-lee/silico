//! The render worker the core starts (ARCHITECTURE.md, Process model). It restarts itself
//! with lavapipe's variables first (`silico_render::lavapipe`), then serves its inherited
//! socket. `--any-adapter` lets it draw on a GPU when there's no lavapipe; by default it
//! draws only on lavapipe. Exits 4 if it can't restart, saying why.

use std::io::Write;
use std::process::ExitCode;

use silico_render::worker::{PLATFORM, RenderAdapter, State};

fn main() -> ExitCode {
    if let Err(problem) = silico_render::lavapipe::reexec_with() {
        let _ = writeln!(std::io::stderr(), "silico-render-worker: {problem}");
        return ExitCode::from(4);
    }
    let policy = if std::env::args_os().skip(1).any(|a| a == "--any-adapter") {
        RenderAdapter::Any
    } else {
        RenderAdapter::Software
    };
    let mut state = State::new(policy);
    silico_worker::run(&PLATFORM, move |_, _, op, _| state.handle(op))
}
