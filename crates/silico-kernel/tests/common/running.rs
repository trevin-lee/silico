//! A worker that says when it has a request, for tests that must act on a request only once
//! it's running: one superseded before then is dropped unrun, and never reaches a worker.

use crate::wait::WAIT;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use silico_core::kernel::KernelRequest;
use silico_core::pool::{KernelPool, Pending};

use super::scripts;

/// A worker that says it started (its first frame, empty, as a worker's is), then, given a
/// request, notes it by writing `marker`, then sleeps without replying until it's killed: so
/// a test knows its request is running, on a worker that had started.
pub fn marking_worker(marker: &Path) -> PathBuf {
    scripts::script(
        "marks-its-request",
        &format!(
            "printf '\\0\\0\\0\\0' >&3\nhead -c 1 <&3 >/dev/null\n: > '{}'\nexec sleep 30",
            marker.display()
        ),
    )
}

/// Submits `request` for the pool's generation, in `span`, and waits until the worker has
/// it (`marker` written). A request superseded only then can't have been dropped unrun.
pub fn submit_running<T: Send + 'static>(
    pool: &KernelPool,
    span: &tracing::Span,
    marker: &Path,
    request: KernelRequest<T>,
) -> Pending<T> {
    let _ = std::fs::remove_file(marker);
    let pending = span.in_scope(|| pool.submit(pool.generation(), request));
    let deadline = Instant::now() + WAIT;
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "the worker never got the request"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    pending
}
