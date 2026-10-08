//! The kernel worker the core starts (ARCHITECTURE.md, Process model). It answers only the
//! production operations; test operations get an "unknown operation" failure.

fn main() -> std::process::ExitCode {
    silico_kernel::worker::run(|_, _, op, _| silico_kernel::worker::handle(op))
}
