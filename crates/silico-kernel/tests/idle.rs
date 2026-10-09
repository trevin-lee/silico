//! Waiting on a worker costs the core no CPU: its threads sleep in poll, both while a
//! worker is idle and while a send waits for a worker that isn't reading. In its own test
//! binary, so the CPU time it measures is only its own.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/scripts.rs"]
mod scripts;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use scripts::script;
use silico_core::worker::Worker;
use silico_protocol::{Blob, Generation, Op};

/// This process's CPU time (user and system), in seconds.
fn cpu_seconds() -> f64 {
    #[cfg(target_os = "linux")]
    {
        // utime and stime, the 14th and 15th fields, in clock ticks.
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let after_name = &stat[stat.rfind(')').unwrap() + 2..];
        let fields: Vec<&str> = after_name.split(' ').collect();
        let ticks = fields[11].parse::<f64>().unwrap() + fields[12].parse::<f64>().unwrap();
        let out = Command::new("getconf").arg("CLK_TCK").output().unwrap();
        let per_second: f64 = String::from_utf8(out.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        ticks / per_second
    }
    #[cfg(not(target_os = "linux"))]
    {
        // [[hours:]minutes:]seconds.fraction
        let out = Command::new("ps")
            .args(["-o", "time=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .trim()
            .split(':')
            .fold(0.0, |total, part| {
                total * 60.0 + part.parse::<f64>().unwrap()
            })
    }
}

#[test]
fn waiting_on_a_worker_costs_no_cpu() {
    // One test, so the two waits don't overlap.
    let echo = |w: &mut Worker, bytes: usize, timeout: Duration| {
        w.call(
            Generation(1),
            Op::Echo {
                data: Blob::inline(vec![0; bytes]),
            },
            timeout,
        )
    };

    // An idle worker: the reply thread waits for the next reply.
    let mut w = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap();
    assert!(echo(&mut w, 1, Duration::from_secs(30)).is_ok());
    let before = cpu_seconds();
    std::thread::sleep(Duration::from_secs(1));
    let idle = cpu_seconds() - before;
    assert!(idle < 0.3, "{idle} s of CPU in 1 s with an idle worker");

    // A send to a worker that never reads: it waits for room until its timeout.
    let mut stuck = Worker::spawn(&script("never-reads", "exec sleep 600")).unwrap();
    let before = cpu_seconds();
    assert!(echo(&mut stuck, 32 * 1024 * 1024, Duration::from_secs(1)).is_err());
    let sending = cpu_seconds() - before;
    // About 0.01 s when it sleeps in poll; a send that spins instead costs a quarter of
    // its timeout or more.
    assert!(
        sending < 0.1,
        "{sending} s of CPU in a 1 s send that couldn't finish"
    );
}
