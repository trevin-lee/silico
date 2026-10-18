//! The `silico` binary starts, and with no command says how it's used and exits 2.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Stdio};

#[test]
fn silico_with_no_command_says_how_its_used() {
    let out = Command::new(env!("CARGO_BIN_EXE_silico"))
        .stdin(Stdio::null())
        .env_remove("SILICO_LOG")
        .output()
        .expect("silico binary should start");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.starts_with("usage: silico check"), "{stderr}");
}
