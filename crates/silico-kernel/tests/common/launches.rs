//! For the pool's back-off tests: workers that log each launch, and the times between
//! them.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a wait for launches may take before it's a hang.
const HANG: Duration = Duration::from_secs(60);

/// as [`epoch_now`] gives them), then runs `then`.
pub fn launch_logger(dir: &Path, then: &str) -> (PathBuf, PathBuf) {
    let script = dir.join("worker.sh");
    let log = dir.join("launches");
    std::fs::write(
        &script,
        format!("#!/bin/sh\n{}\n{then}\n", log_launch(&log)),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

/// A shell line that logs the time to `log`, in seconds since the epoch.
pub fn log_launch(log: &Path) -> String {
    format!(
        "echo \"$(perl -MTime::HiRes=time -e 'print time') $$\" >> '{}'",
        log.display()
    )
}

/// The launches logged to `log` so far, earliest first: when, and the process ID.
pub fn launches(log: &Path) -> Vec<(f64, u32)> {
    let mut launches: Vec<(f64, u32)> = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let (time, pid) = l.trim().split_once(' ').unwrap();
            (time.parse().unwrap(), pid.parse().unwrap())
        })
        .collect();
    launches.sort_by(|a, b| a.0.total_cmp(&b.0));
    launches
}

/// The launch times logged to `log` so far, earliest first.
pub fn launch_times(log: &Path) -> Vec<f64> {
    launches(log).into_iter().map(|(time, _)| time).collect()
}

/// Waits for `log` to hold `n` launches logged after `after`, and gives them, earliest
/// first; failing only after [`HANG`].
pub fn wait_launches_after(log: &Path, after: f64, n: usize) -> Vec<f64> {
    let deadline = Instant::now() + HANG;
    loop {
        let times: Vec<f64> = launch_times(log)
            .into_iter()
            .filter(|&t| t > after)
            .collect();
        if times.len() >= n {
            return times;
        }
        assert!(
            Instant::now() < deadline,
            "{} launches after {HANG:?}, not {n}: at {times:?}",
            times.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits for `log` to hold `n` launches, and gives them, earliest first.
pub fn wait_launches(log: &Path, n: usize) -> Vec<f64> {
    wait_launches_after(log, f64::NEG_INFINITY, n)
}

/// Whether process `pid` has ended: exited (a zombie), or reaped (`ps` may still list one
/// The gaps between `times`.
pub fn gaps(times: &[f64]) -> Vec<f64> {
    times.windows(2).map(|w| w[1] - w[0]).collect()
}

/// Checks that each gap between `times` is longer than the wait `waits` gives it. A slot
/// waits from the idle check that sees its worker's end, which comes after the worker has
/// logged its launch: so the gap from that launch to the next is longer than the wait,
/// however late a loaded machine runs the checks.
pub fn assert_gaps_exceed(times: &[f64], waits: &[f64]) {
    let gaps = gaps(times);
    assert!(gaps.len() >= waits.len(), "launches at {times:?}");
    for (k, (gap, wait)) in gaps.iter().zip(waits).enumerate() {
        assert!(
            gap > wait,
            "gap {k} is {gap:.3} s, not longer than its {wait} s wait: gaps {gaps:?}"
        );
    }
}
