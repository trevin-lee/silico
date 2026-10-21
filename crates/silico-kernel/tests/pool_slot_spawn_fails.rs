//! A pool that can't start a slot's thread, the system short of threads: the slots already
//! started take the waiting requests in turn; with none started, they fail with the
//! shortage; either way the slot starts later, once threads are to be had. Alone in its
//! binary: it runs the process out of threads (macOS) or lowers the user's limit on them
//! (Linux).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_protocol::{DebugOp, Generation};

const WAIT: Duration = Duration::from_secs(10);

fn echo(byte: u8) -> KernelRequest<Blob> {
    KernelRequest::echo(Blob::inline(vec![byte]))
}

fn pool(size: usize) -> KernelPool {
    KernelPool::start_on_demand(
        Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker")),
        size,
    )
    .unwrap()
}

/// No thread can start while this lives: Linux counts threads against the user's process
/// limit, so it lowers that to none.
#[cfg(target_os = "linux")]
struct NoThreads(rustix::process::Rlimit);

#[cfg(target_os = "linux")]
impl NoThreads {
    fn new() -> Self {
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        let saved = getrlimit(Resource::Nproc);
        setrlimit(
            Resource::Nproc,
            Rlimit {
                current: Some(0),
                maximum: saved.maximum,
            },
        )
        .unwrap();
        NoThreads(saved)
    }
}

#[cfg(target_os = "linux")]
impl Drop for NoThreads {
    fn drop(&mut self) {
        let _ = rustix::process::setrlimit(rustix::process::Resource::Nproc, self.0.clone());
    }
}

/// No thread can start while this lives: macOS doesn't count threads against the process
/// limit, but caps a process's threads, so it starts parked threads until no more start.
#[cfg(target_os = "macos")]
struct NoThreads {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    parked: Vec<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "macos")]
impl NoThreads {
    fn new() -> Self {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let mut parked = Vec::new();
        // kern.num_taskthreads is 12,288 on macOS 15 and later; well past it, something's
        // wrong.
        for _ in 0..100_000 {
            let stopped = Arc::clone(&stop);
            let spawned = std::thread::Builder::new()
                .stack_size(16 * 1024)
                .spawn(move || {
                    while !stopped.load(Ordering::SeqCst) {
                        std::thread::park();
                    }
                });
            match spawned {
                Ok(thread) => parked.push(thread),
                Err(e) => {
                    assert_eq!(
                        e.raw_os_error(),
                        Some(rustix::io::Errno::AGAIN.raw_os_error()),
                        "{e}"
                    );
                    return NoThreads { stop, parked };
                }
            }
        }
        panic!("100,000 threads started: the process has no limit on them");
    }
}

#[cfg(target_os = "macos")]
impl Drop for NoThreads {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        for thread in self.parked.drain(..) {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[test]
fn a_slot_whose_thread_cant_start_leaves_its_requests_to_the_slots_started_or_fails_them() {
    let busy = pool(2);
    // One slot, so the one that couldn't start must be the one that starts later.
    let idle = pool(1);

    // `busy`'s first slot takes a sleep: once its worker exists, the slot is running it.
    let sleeping = busy.submit(
        Generation(1),
        KernelRequest::debug(DebugOp::Sleep { millis: 2_000 }),
    );
    let started = Instant::now();
    while busy.pids().is_empty() {
        assert!(started.elapsed() < WAIT, "the sleep's worker never started");
        std::thread::sleep(Duration::from_millis(5));
    }

    let no_threads = NoThreads::new();
    assert!(
        std::thread::Builder::new().spawn(|| {}).is_err(),
        "a thread still starts"
    );

    // Every started slot busy, the next request needs another, whose thread can't start:
    // the request waits for the slot that's running, which takes it once the sleep is done.
    let waiting = busy.submit(Generation(1), echo(1));
    sleeping
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the sleep never ended"))
        .done()
        .expect("superseded")
        .unwrap();
    let answer = waiting
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the request waited: the slot running never took it"))
        .done()
        .expect("superseded")
        .unwrap_or_else(|e| panic!("the request failed, though a slot was started: {e}"));
    assert_eq!(answer, Blob::inline(vec![1]));
    assert_eq!(busy.pids().len(), 1, "a second worker started");

    // No slot started, and none can start: the request fails with the shortage, not waits.
    let err = idle
        .submit(Generation(1), echo(2))
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the request waited for a slot that can't start"))
        .done()
        .expect("superseded")
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ResourceExhausted, "{err}");
    assert!(idle.pids().is_empty());

    // Threads to be had again, the slot starts: the next request is answered.
    drop(no_threads);
    let answer = idle
        .submit(Generation(1), echo(3))
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the request waited: the slot never started again"))
        .done()
        .expect("superseded")
        .unwrap();
    assert_eq!(answer, Blob::inline(vec![3]));
    assert_eq!(idle.pids().len(), 1);
}
