//! silico-view runs the M0 parts through a real kernel worker and logs them to Rerun.
//! Saving to a file needs no viewer, so the test checks that path end to end.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

/// The kernel worker built by this workspace (the kernel's tests build it).
fn worker() -> PathBuf {
    let worker =
        PathBuf::from(env!("CARGO_BIN_EXE_silico-view")).with_file_name("silico-kernel-worker");
    assert!(
        worker.exists(),
        "{} is missing: build the workspace (pixi run test does)",
        worker.display()
    );
    worker
}

#[test]
fn the_parts_are_built_and_saved() {
    let file =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("view-{}.rrd", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_silico-view"))
        .env("SILICO_KERNEL_WORKER", worker())
        .args(["--save", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let size = std::fs::metadata(&file).unwrap().len();
    std::fs::remove_file(&file).unwrap();
    // Three meshes with their edges: tens of kilobytes, not an empty recording.
    assert!(size > 10_000, "only {size} bytes saved");
}

#[test]
fn bad_arguments_are_refused() {
    for args in [&["--save"][..], &["--bogus", "x"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_silico-view"))
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{args:?} was accepted");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("invalid_input"),
            "{args:?}"
        );
    }
}

#[test]
fn a_path_or_viewer_that_wont_work_is_the_users_input() {
    let missing = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-such-dir/x.rrd");
    for args in [
        vec!["--save".to_owned(), missing.display().to_string()],
        vec!["--viewer".to_owned(), "/nonexistent/rerun".to_owned()],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_silico-view"))
            .env("SILICO_KERNEL_WORKER", worker())
            .args(&args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains("invalid_input"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn help_prints_usage() {
    // Without starting a worker: it works before one is built.
    let out = Command::new(env!("CARGO_BIN_EXE_silico-view"))
        .env("SILICO_KERNEL_WORKER", "/nonexistent/silico-kernel-worker")
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("usage: silico-view"));
}
