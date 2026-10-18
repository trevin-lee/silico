//! The worker pool's idle slots, with worker binaries that are shell scripts: what a slot
//! does about a worker that never says it started needs no kernel. The pool's tests with
//! real workers are silico-kernel's (tests/pool.rs, tests/adversarial_pool.rs).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use silico_core::pool::{IDLE_CHECK, KernelPool};

/// Long enough for any wait here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

/// A directory of its own for one test, removed after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("silico-pool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A worker script that logs each launch to `launches` in `dir` (the time, in seconds since
/// the epoch, and its process ID), then runs `then`.
fn launch_logger(dir: &Path, then: &str) -> (PathBuf, PathBuf) {
    let script = dir.join("worker.sh");
    let log = dir.join("launches");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$(perl -MTime::HiRes=time -e 'print time') $$\" >> '{}'\n{then}\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

/// The launches logged to `log` once there are `n`, earliest first: when, and the process
/// ID; failing only after [`HANG`].
fn wait_launches(log: &Path, n: usize) -> Vec<(f64, i32)> {
    let deadline = Instant::now() + HANG;
    loop {
        let mut launches: Vec<(f64, i32)> = std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let (time, pid) = l.trim().split_once(' ').unwrap();
                (time.parse().unwrap(), pid.parse().unwrap())
            })
            .collect();
        launches.sort_by(|a, b| a.0.total_cmp(&b.0));
        if launches.len() >= n {
            return launches;
        }
        assert!(
            Instant::now() < deadline,
            "{} launches of {n}",
            launches.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Now, in seconds since the epoch: the clock the launch log is in.
fn epoch_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// Kills process `pid` with SIGKILL, as from outside the pool.
fn kill9(pid: i32) {
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn a_slot_whose_workers_are_killed_twice_running_before_they_start_waits_before_the_next() {
    // Found by the milestone's mutation run: a worker running at an idle check, though it
    // hadn't said it started, was noted as started (the check was `started || running`, or
    // ran once whatever the worker), which forgot that the slot's last worker was killed
    // before it started too, and passed every test. A slot replaces a worker killed
    // (SIGKILL) before it started at once, unless the slot's last worker was too: then
    // after a wait of an idle check, as for a binary the system kills at launch (pool.rs,
    // `tend`). Each worker here runs on, never saying it started, through an idle check
    // before it's killed.
    let dir = Scratch::new("killed-unstarted");
    let (script, log) = launch_logger(&dir.0, "exec sleep 60");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    let alive = IDLE_CHECK + IDLE_CHECK / 2;
    let first = wait_launches(&log, 1)[0].1;
    std::thread::sleep(alive);
    kill9(first);
    // Its last worker wasn't killed: replaced at once.
    let second = wait_launches(&log, 2)[1].1;
    std::thread::sleep(alive);
    let killed = epoch_now();
    kill9(second);
    // Its last worker was killed before it started too: the next comes a check later.
    let third = wait_launches(&log, 3)[2].0;
    assert!(
        third - killed >= IDLE_CHECK.as_secs_f64() - 0.01,
        "replaced {:.3} s after the second kill",
        third - killed
    );
    drop(pool);
}
