//! Waiting on a worker costs the core no CPU: its threads sleep in poll, both while a
//! worker is idle and while a send waits for a worker that isn't reading. In its own test
//! binary, so the CPU time it measures is only its own.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/scripts.rs"]
mod scripts;

use std::path::Path;
use std::time::Duration;

use scripts::script;
use silico_core::worker::Worker;
use silico_protocol::{Blob, Generation, Op};

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

    // An idle worker: the reply thread waits for the next reply. Load can't make a sleeping
    // thread cost CPU, only take it from one that spins.
    let mut w = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap();
    assert!(echo(&mut w, 1, Duration::from_secs(30)).is_ok());
    // This process's CPU time: every thread's.
    let before = cpu_time::ProcessTime::now();
    std::thread::sleep(Duration::from_secs(1));
    let idle = before.elapsed();
    assert!(
        idle < Duration::from_millis(300),
        "{idle:?} of CPU in 1 s with an idle worker"
    );

    // A send to a worker that never reads: it waits for room until its timeout, asleep in
    // poll. What it costs besides setting up its 32 MiB doesn't grow with how long it
    // waits, so a send that waits 4 s costs no more than one that waits 1 s (about 0.01 s
    // each); a send that spins instead costs a quarter of each second it waits or more,
    // 0.75 s more for the longer. The difference is the measure: setting up the 32 MiB
    // costs more on a loaded machine short of memory, the same for both.
    let send = |timeout: Duration| {
        let mut stuck = Worker::spawn(&script("never-reads", "exec sleep 600")).unwrap();
        let before = cpu_time::ProcessTime::now();
        assert!(echo(&mut stuck, 32 * 1024 * 1024, timeout).is_err());
        before.elapsed()
    };
    let short = send(Duration::from_secs(1));
    let long = send(Duration::from_secs(4));
    // And loosely, on its own: a burst of spinning the same for both shows here.
    assert!(
        short < Duration::from_millis(500),
        "a send that couldn't finish cost {short:?} of CPU waiting 1 s"
    );
    assert!(
        long.saturating_sub(short) < Duration::from_millis(100),
        "a send that couldn't finish cost {short:?} of CPU waiting 1 s, {long:?} waiting 4 s"
    );
}
