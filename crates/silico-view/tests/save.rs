//! silico-view makes the kernel's parts through a real kernel worker and logs them to Rerun.
//! Saving to a file needs no viewer, so the test checks that path end to end.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

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

/// silico-view, set up so that no test can start a real Rerun viewer, even with code that
/// has lost `--save` or `--viewer`'s path: a mutant that dropped the path once opened one
/// on the maintainer's desktop, from the `rerun` on PATH. Its PATH holds only a stand-in
/// `rerun` that leaves a mark and fails, and it runs in a directory of its own, where
/// Rerun's debug-build fallback (`target/debug/rerun`) finds nothing; [`View::output`]
/// fails the test if the mark is there. (A viewer already listening on Rerun's port, 9876,
/// is still connected to rather than started: close it before running these tests.)
struct View {
    dir: PathBuf,
    command: Command,
}

fn view() -> View {
    static RUNS: AtomicU32 = AtomicU32::new(0);
    let run = RUNS.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("view-run-{}-{run}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stand_in = bin.join("rerun");
    std::fs::write(
        &stand_in,
        format!(
            "#!/bin/sh\n: > '{}'\nexit 1\n",
            dir.join("viewer-started").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_silico-view"));
    command
        .current_dir(&dir)
        .env("PATH", &bin)
        .stdin(Stdio::null());
    View { dir, command }
}

impl View {
    /// Its output, having checked it started no viewer.
    fn output(&mut self) -> Output {
        let out = self.command.output().unwrap();
        assert!(
            !self.dir.join("viewer-started").exists(),
            "silico-view started a Rerun viewer: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}

impl Drop for View {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn the_parts_are_built_and_saved() {
    let file =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("view-{}.rrd", std::process::id()));
    let mut run = view();
    run.command
        .env("SILICO_KERNEL_WORKER", worker())
        .args(["--save", file.to_str().unwrap()]);
    let out = run.output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let size = std::fs::metadata(&file).unwrap().len();
    std::fs::remove_file(&file).unwrap();
    // Seven meshes with their edges: tens of kilobytes, not an empty recording.
    assert!(size > 10_000, "only {size} bytes saved");
}

#[test]
fn the_viewer_finds_the_worker_beside_it() {
    // As `pixi run view` runs it: no SILICO_KERNEL_WORKER, the worker built next to it.
    let file = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("view-beside-{}.rrd", std::process::id()));
    let mut run = view();
    run.command
        .env_remove("SILICO_KERNEL_WORKER")
        .args(["--save", file.to_str().unwrap()]);
    let out = run.output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_file(&file).unwrap();
}

#[test]
fn bad_arguments_are_refused() {
    for args in [&["--save"][..], &["--bogus", "x"]] {
        let mut run = view();
        run.command.args(args);
        let out = run.output();
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
        let mut run = view();
        run.command
            .env("SILICO_KERNEL_WORKER", worker())
            .args(&args);
        let out = run.output();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains("invalid_input"),
            "{args:?}: {stderr}"
        );
        // The error names the path given, so it's that path the viewer was started from.
        // (Found by mutation: without the path, Rerun runs the `rerun` on PATH, here the
        // stand-in, which fails too, so the run failed as before.)
        assert!(stderr.contains(&args[1]), "{args:?}: {stderr}");
    }
}

#[test]
fn help_prints_usage() {
    // Without starting a worker: it works before one is built.
    let mut run = view();
    run.command
        .env("SILICO_KERNEL_WORKER", "/nonexistent/silico-kernel-worker")
        .arg("--help");
    let out = run.output();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("usage: silico-view"));
}

#[test]
fn logs_are_shown_when_asked_for() {
    // The viewer logs to stderr like every Silico process, so a worker's trouble is visible.
    let file = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("view-logs-{}.rrd", std::process::id()));
    let mut run = view();
    run.command
        .env("SILICO_KERNEL_WORKER", worker())
        .env("SILICO_LOG", "silico_core=debug")
        .args(["--save", file.to_str().unwrap()]);
    let out = run.output();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    std::fs::remove_file(&file).unwrap();
    assert!(stderr.contains("started kernel worker"), "{stderr}");
}
