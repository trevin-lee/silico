//! The sketch worker the core starts (ARCHITECTURE.md, Process model). It answers only the
//! production operations; test operations get an "unknown operation" failure.

fn main() -> std::process::ExitCode {
    silico_worker::run(&silico_sketch::worker::PLATFORM, |_, _, op, _| {
        silico_sketch::worker::handle(op)
    })
}
