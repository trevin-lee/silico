//! Waiting on a worker costs the core no CPU: its threads sleep in poll, both while a
//! worker is idle and while a send waits for a worker that isn't reading. In its own test
//! binary, so the CPU time it measures is only its own.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::scripts;

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
    // poll. What it costs besides setting up its 8 MiB and a byte doesn't grow with how long
    // it waits: twice the core's 4 MiB send buffer (`worker::SOCKET_BUFFER`) and a byte,
    // past what Linux, which doubles the buffer asked for, holds before a send waits, and
    // past macOS's, which holds the sender's 4 MiB. So, measured against a send that waits
    // 10 ms, one that waits 1 s costs nothing more, and one that waits 4 s nothing more
    // again (about 0.01 s each). A send that spins instead costs a quarter of each second it
    // waits or more: 0.25 s more for the 1 s wait, 0.75 s more for the 4 s one; and a burst
    // of spinning at the start of each wait shows against the 10 ms one. The differences are
    // the measure: what setting up the payload costs grows on a loaded machine short of
    // memory, or one whose CPU time stretches under contention (a gate's 32 MiB once cost
    // 0.86 s), but the same for each send.
    let send = |timeout: Duration| {
        let mut stuck = Worker::spawn(&script("never-reads", "exec sleep 600")).unwrap();
        let before = cpu_time::ProcessTime::now();
        assert!(echo(&mut stuck, 8 * 1024 * 1024 + 1, timeout).is_err());
        before.elapsed()
    };
    let brief = send(Duration::from_millis(10));
    let short = send(Duration::from_secs(1));
    let long = send(Duration::from_secs(4));
    assert!(
        short.saturating_sub(brief) < Duration::from_millis(200),
        "a send that couldn't finish cost {short:?} of CPU waiting 1 s, {brief:?} waiting 10 ms"
    );
    assert!(
        long.saturating_sub(short) < Duration::from_millis(100),
        "a send that couldn't finish cost {short:?} of CPU waiting 1 s, {long:?} waiting 4 s"
    );
}
