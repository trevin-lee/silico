//! The sketch worker the core starts (ARCHITECTURE.md, Process model). It answers only the
//! production operations; test operations get an "unknown operation" failure.

fn main() -> std::process::ExitCode {
    let mut state = silico_sketch::worker::State::default();
    silico_worker::run(&silico_sketch::worker::PLATFORM, move |_, _, op, _| {
        state.handle(op)
    })
}
