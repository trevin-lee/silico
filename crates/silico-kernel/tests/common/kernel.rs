//! The core's typed layer over a fresh kernel worker.

use std::path::Path;

use silico_core::kernel::KernelWorker;
use silico_core::step_worker::StepWorker;

/// A newly started kernel worker.
pub fn kernel() -> KernelWorker {
    KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap()
}

/// A newly started STEP worker.
#[allow(dead_code)]
pub fn step() -> StepWorker {
    StepWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-step-worker"))).unwrap()
}
