//! The `silico` command-line client.

use std::process::ExitCode;

fn main() -> ExitCode {
    silico_core::logging::init();
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "silico starting");
    ExitCode::SUCCESS
}
