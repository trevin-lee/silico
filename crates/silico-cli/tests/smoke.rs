//! The `silico` binary starts and exits cleanly.

use std::process::{Command, Stdio};

#[test]
fn silico_binary_runs() {
    let status = Command::new(env!("CARGO_BIN_EXE_silico"))
        .stdin(Stdio::null())
        .status()
        .expect("silico binary should start");
    assert!(status.success());
}
