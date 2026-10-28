//! silico-kernel's pools tests, as one test program: each file a module, its tests
//! as they were, the modules they share loaded once: fewer programs to build and link than
//! one a file, for every change (each mutant's build most of all).
//!
//! Some install the process's global tracing subscriber, or measure the process's CPU time:
//! each test needs a process of its own, as nextest runs each in (`pixi run test`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/lateness.rs"]
mod lateness;
#[path = "../../silico-core/tests/common/test_programs.rs"]
mod test_programs;
#[path = "common/wait.rs"]
mod wait;

// Shared by the files below, each loaded once.
#[path = "common/launches.rs"]
mod launches;
#[path = "common/running.rs"]
mod running;
#[path = "common/scripts.rs"]
mod scripts;

#[path = "adversarial_worker_logs.rs"]
mod adversarial_worker_logs;
#[path = "adversarial_worker_start.rs"]
mod adversarial_worker_start;
#[path = "idle.rs"]
mod idle;
#[path = "pool_backoff.rs"]
mod pool_backoff;
#[path = "pool_blocked_log.rs"]
mod pool_blocked_log;
#[path = "pool_logs.rs"]
mod pool_logs;
#[path = "pool_slot_panics.rs"]
mod pool_slot_panics;
#[path = "pool_slot_panics_always.rs"]
mod pool_slot_panics_always;
#[path = "pool_slot_panics_in_run.rs"]
mod pool_slot_panics_in_run;
#[path = "pool_slot_panics_running.rs"]
mod pool_slot_panics_running;
#[path = "pool_tracing.rs"]
mod pool_tracing;

/// Every file in tests/ is built: this crate lists its test programs (`autotests = false`),
/// so a new file must be added as a module of one, or as one, or it would never run.
#[test]
fn every_test_file_is_built() {
    test_programs::assert_every_test_file_is_built(env!("CARGO_MANIFEST_DIR"));
}
