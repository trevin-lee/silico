//! break-it for the start code through the binary: workers that can't be had, named by
//! `SILICO_KERNEL_WORKER` and `SILICO_SKETCH_WORKER`, through `check --json`. The contract:
//! a worker that never reached its first frame is `worker_unavailable` (error.rs), and a
//! command that couldn't run exits 2 (docs/cli.md); one input, one answer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/run.rs"]
mod run;
#[path = "../../silico-core/tests/common/share.rs"]
mod share;

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
            // 20 s, and room for what load stretches, the processes' starts (some 3 s of work
            // each slowdown over a quiet machine, `Share`), but not the fixed waits a broken
            // check would add, which load doesn't shorten; never 50 s, so a run that waits
            // 50 to 60 s is named here (past 60 s, the run's own deadline stops it, as one
            // waiting out the kernel's 60 s request timeout on a lingering worker would be).
            let share = share::Share::start();
            let started = std::time::Instant::now();
            let (status, stdout) = check_json(&dir, text, &[(variable, value.as_os_str())]);
            let (wall, slowdown) = (started.elapsed(), share.finish().slowdown);
            let case = format!("{variable} {what}");
            if status != Some(2) || !stdout.contains("worker_unavailable") {
                wrong.push(format!("{case}: exit {status:?}: {stdout}"));
            }
            let bound = (std::time::Duration::from_secs(20)
                + std::time::Duration::from_secs(3).mul_f64(slowdown - 1.0))
            .min(std::time::Duration::from_secs(50));
            if wall >= bound {
                wrong.push(format!(
                    "{case}: took {wall:?}, past {bound:?}, the machine {slowdown:.2} times slower \
                     than a quiet one"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `silico check --json part.silico` run as `exe`, in `dir`, with no worker named by a
/// variable: each found beside the executable or not at all. Its exit code and stdout.
fn check_as(exe: &Path, dir: &Path, text: &str) -> (Option<i32>, String) {
    std::fs::write(dir.join("part.silico"), text).unwrap();
    let mut c = Command::new(exe);
    c.args(["check", "--json", "part.silico"])
        .current_dir(dir)
        .env_remove("SILICO_KERNEL_WORKER")
        .env_remove("SILICO_SKETCH_WORKER")
        .env_remove("SILICO_RENDER_WORKER")
        .env_remove("SILICO_STEP_WORKER")
        .env("XDG_CACHE_HOME", dir.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    let out = run::output(c);
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn a_silico_reached_through_a_link_finds_the_workers_beside_its_build() {
    // As a `silico` on PATH is: a link, in a directory of its own, to the build's. Its
    // workers are beside the build, never beside the link. This fails without the fix on
    // macOS, whose running executable is the path as run; Linux's is already resolved, and
    // the core's tests of the lookup itself hold it there.
    let dir = scratch("linked");
    let link = dir.join("silico");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_silico"), &link).unwrap();
    let (status, stdout) = check_as(&link, &dir, BOX);
    assert_eq!(status, Some(0), "{stdout}");
    assert!(stdout.contains("\"ok\":true"), "{stdout}");
    // A sketch, which the sketch worker solves, as well.
    let (status, stdout) = check_as(&link, &dir, SKETCH);
    assert_eq!(status, Some(0), "{stdout}");
}

#[test]
fn a_silico_copied_alone_names_where_it_looked_for_its_worker() {
    // A copy has no workers beside it: the kernel fault names the directory looked in and
    // the worker looked for, and how to put it right.
    let dir = scratch("copied");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let copy = bin.join("silico");
    std::fs::copy(env!("CARGO_BIN_EXE_silico"), &copy).unwrap();
    let bin = std::fs::canonicalize(&bin).unwrap();
    // Run by its resolved path, so on macOS (whose running executable is the path as run) a
    // scratch directory reached through a link adds no "run as".
    let copy = bin.join("silico");
    let (status, stdout) = check_as(&copy, &dir, BOX);
    assert_eq!(status, Some(2), "{stdout}");
    assert_eq!(code(&stdout), "worker_unavailable", "{stdout}");
    let message = format!(
        "no kernel worker in {}: Silico looks for silico-kernel-worker beside its own executable, {}",
        bin.display(),
        bin.join("silico").display()
    );
    // The whole message: run as itself, nothing after it.
    assert!(
        stdout.contains(&format!("\"{message}\"")),
        "{message}\nin {stdout}"
    );
    let suggestion = format!(
        "build it (cargo build -p silico-kernel --bin silico-kernel-worker) or put it in {}, or name it in SILICO_KERNEL_WORKER",
        bin.display()
    );
    assert!(stdout.contains(&suggestion), "{suggestion}\nin {stdout}");
}
