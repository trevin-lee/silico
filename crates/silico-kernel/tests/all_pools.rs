//! silico-kernel's pools tests, as one test program: each file a module, its tests
//! as they were, the modules they share loaded once: fewer programs to build and link than
//! one a file, for every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/launches.rs"]
mod launches;
#[path = "common/scripts.rs"]
mod scripts;

#[path = "adversarial_worker_logs.rs"]
mod adversarial_worker_logs;
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
