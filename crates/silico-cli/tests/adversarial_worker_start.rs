//! break-it for the start code through the binary: workers that can't be had, named by
//! `SILICO_KERNEL_WORKER` and `SILICO_SKETCH_WORKER`, through `check --json`. The contract:
//! a worker that never reached its first frame is `worker_unavailable` (error.rs), and a
//! command that couldn't run exits 2 (docs/cli.md); one input, one answer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/run.rs"]
mod run;

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One sketch, a line, and nothing built from it: a solve of the sketch worker's.
const SKETCH: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

const BOX: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adv-cli-start-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn kernel_worker() -> PathBuf {
    let worker = Path::new(env!("CARGO_BIN_EXE_silico")).with_file_name("silico-kernel-worker");
    assert!(worker.exists(), "{} isn't built", worker.display());
    worker
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// `silico check --json part.silico` in `dir`, with `env` set: its exit code and stdout.
fn check_json(dir: &Path, text: &str, env: &[(&str, &std::ffi::OsStr)]) -> (Option<i32>, String) {
    std::fs::write(dir.join("part.silico"), text).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_silico"));
    c.args(["check", "--json", "part.silico"])
        .current_dir(dir)
        .env("SILICO_KERNEL_WORKER", kernel_worker())
        .env("XDG_CACHE_HOME", dir.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    run::ready(&mut c);
    let out = run::output(c);
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// The code of `check --json`'s error line.
fn code(stdout: &str) -> String {
    let at = stdout
        .find("\"code\":")
        .unwrap_or_else(|| panic!("no code in {stdout}"));
    let rest = &stdout[at + 7..];
    let rest = rest.trim_start().trim_start_matches('"');
    rest[..rest.find('"').unwrap()].to_owned()
}

#[test]
fn a_sketch_worker_that_exits_at_launch_is_one_code_every_run() {
    // A sketch worker that exits as it starts (never a first frame) is worker_unavailable,
    // run after run. Through the API the code flips to worker_crashed when the solve comes
    // after the worker has exited (silico-kernel's adversarial_worker_start: a solve begun
    // with a stopper words an exited worker as a crash, sketch.rs `solve`); through the
    // binary the solve comes first so far. This holds that to every run.
    let dir = scratch("sketch-exits");
    let exits = script(&dir, "sketch-exits", "exit 3");
    let mut codes = BTreeSet::new();
    for run in 0..12 {
        let (status, stdout) =
            check_json(&dir, SKETCH, &[("SILICO_SKETCH_WORKER", exits.as_os_str())]);
        assert_eq!(status, Some(2), "run {run}: {stdout}");
        codes.insert(code(&stdout));
    }
    assert_eq!(
        codes.into_iter().collect::<Vec<_>>(),
        ["worker_unavailable"],
        "one install, one code"
    );
}

#[test]
fn every_unusable_worker_variable_is_unavailable_exit_2() {
    let dir = scratch("variables");
    let fifo = dir.join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let zero = dir.join("zero");
    std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
    let dangling = dir.join("dangling");
    std::os::unix::fs::symlink(dir.join("nothing"), &dangling).unwrap();
    let lingers = script(&dir, "closes-and-lingers", "exec 3>&-\nexec sleep 600");
    let empty_file = dir.join("empty-file");
    std::fs::write(&empty_file, "").unwrap();
    std::fs::set_permissions(&empty_file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let trailing_slash = PathBuf::from(format!("{}/", kernel_worker().display()));
    let mut wrong = Vec::new();
    for (variable, text) in [
        ("SILICO_KERNEL_WORKER", BOX),
        ("SILICO_SKETCH_WORKER", SKETCH),
    ] {
        for (what, value) in [
            ("empty", PathBuf::new()),
            ("a FIFO", fifo.clone()),
            ("a link to /dev/zero", zero.clone()),
            ("a dangling link", dangling.clone()),
            ("an empty executable", empty_file.clone()),
            ("a file named with a trailing slash", trailing_slash.clone()),
            ("closes its socket and lingers", lingers.clone()),
        ] {
            let started = std::time::Instant::now();
            let (status, stdout) = check_json(&dir, text, &[(variable, value.as_os_str())]);
            let case = format!("{variable} {what}");
            if status != Some(2) || !stdout.contains("worker_unavailable") {
                wrong.push(format!("{case}: exit {status:?}: {stdout}"));
            }
            if started.elapsed() > std::time::Duration::from_secs(20) {
                wrong.push(format!("{case}: took {:?}", started.elapsed()));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
