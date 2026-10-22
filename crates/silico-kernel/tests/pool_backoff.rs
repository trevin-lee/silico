//! A slot's back-off after its worker ends at launch: what it decides each time (to start
//! another at once, or after a wait, and how long), as the pool says it in its span's
//! events, and launches no sooner than those waits allow. The decisions, not the times
//! between launches, are what's checked to the second, so a stalled machine can't fail
//! these (a VM stall of 10 s once did); each test runs alone in its process (nextest), so
//! the subscriber it installs sees only its pool.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/launches.rs"]
mod launches;

use launches::{Decision, Decisions, assert_gaps_exceed, gaps, launch_logger, wait_launches};
use silico_core::pool::KernelPool;

/// A fresh directory for one test's worker and its launches.
fn scratch_dir(what: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pool-backoff-{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_idle_slot_whose_worker_is_killed_at_launch_backs_off_after_the_second_kill() {
    // SIGKILL before the worker starts, every time, as macOS kills a binary whose code
    // signature is broken. The first may be a kill from outside, so it's replaced at once;
    // after the second in a row the slot waits as for a binary that can't start: 1 s, then
    // 2, 4, and 8 s. Not waiting from the first kill (1, 2, 4, 8, 16 s), nor at once each
    // time. And each launch after a wait comes no sooner than it allows.
    let decisions = Decisions::record();
    let dir = scratch_dir("killed-at-launch");
    let (script, log) = launch_logger(&dir, "kill -KILL $$");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    let times = wait_launches(&log, 6);
    drop(pool);
    use Decision::{AtOnce, Wait};
    assert_eq!(
        decisions.of_slot(0, 5),
        [AtOnce, Wait(1000), Wait(2000), Wait(4000), Wait(8000)]
    );
    assert_gaps_exceed(&times[1..6], &[1.0, 2.0, 4.0, 8.0]);
}

#[test]
fn a_kill_at_launch_after_an_exit_of_its_own_is_taken_as_from_outside() {
    // The first three launches exit on their own (code 3) before they start; every later one
    // is killed (SIGKILL) before it starts. Each exit is waited on, the wait doubling (1, 2,
    // 4 s); the first kill follows an exit, not a kill, so it's taken as from outside and
    // replaced at once; the second in a row is waited on, its wait doubled from the last
    // exit's 4 s to 8 s. And each launch after a wait comes no sooner than it allows.
    let decisions = Decisions::record();
    let dir = scratch_dir("exit-then-kills");
    let (script, log) = launch_logger(
        &dir,
        &format!(
            "for n in 1 2 3; do mkdir '{}'/exit$n 2>/dev/null && exit 3; done\nkill -KILL $$",
            dir.display()
        ),
    );
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    let times = wait_launches(&log, 6);
    drop(pool);
    use Decision::{AtOnce, Wait};
    assert_eq!(
        decisions.of_slot(0, 5),
        [Wait(1000), Wait(2000), Wait(4000), AtOnce, Wait(8000)]
    );
    let times = &times[..6];
    assert_gaps_exceed(&times[..4], &[1.0, 2.0, 4.0]);
    assert!(
        gaps(times)[4] > 8.0,
        "the second kill wasn't waited on: gaps {:?}",
        gaps(times)
    );
}
