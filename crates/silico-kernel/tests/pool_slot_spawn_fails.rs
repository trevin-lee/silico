//! A pool that can't start a slot's thread, the system short of threads: the slots already
//! started take the waiting requests in turn; with none started, they fail with the
//! shortage; either way the slot starts later, once threads are to be had.
//!
//! Alone in its binary, and in nextest's `thread-exhausting` group (one at a time), since it
//! takes away its own process's threads:
//! - On Linux it lowers its own `RLIMIT_NPROC`, which Linux checks a thread's start against.
//!   The limit is the process's own: it's checked against the user's count of processes
//!   and threads, but only for this process's own threads and forks, so the user's other
//!   processes keep theirs. The CI runners are single-use, and their user isn't root (root
//!   would pass the limit, which the test checks).
//! - On macOS, where that limit counts only processes, it starts parked threads up to its
//!   own process's limit on threads (`kern.num_taskthreads`, 12,288), and no further: it
//!   lowers no limit and starts no process, so the per-user limits (`kern.maxprocperuid`)
//!   are untouched. It refuses to run if a process's limit is over a quarter of the
//!   system's threads (`kern.num_threads`, 61,440). Every thread it parks is released and
//!   joined before it returns, whether it passes or fails.
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

/// No thread can start while this lives: Linux checks a thread's start against the process's
/// `RLIMIT_NPROC`, so it lowers this process's own to none, and puts it back on drop.
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
        let _ = rustix::process::setrlimit(rustix::process::Resource::Nproc, self.0);
    }
}

/// No thread can start while this lives: macOS caps a process's threads, so it starts
/// parked threads up to the cap. Dropping it, which a panic while it's being made does too,
/// releases and joins every one.
#[cfg(target_os = "macos")]
struct NoThreads {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    parked: Vec<std::thread::JoinHandle<()>>,
}

/// A `sysctl` value.
#[cfg(target_os = "macos")]
fn sysctl(name: &str) -> usize {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .output()
        .unwrap();
    assert!(out.status.success(), "sysctl {name}: {out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[cfg(target_os = "macos")]
impl NoThreads {
    fn new() -> Self {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let per_process = sysctl("kern.num_taskthreads");
        let system = sysctl("kern.num_threads");
        assert!(
            per_process <= system / 4,
            "a process may have {per_process} threads of the system's {system}: running it \
             out of them would leave too few for the rest of the system"
        );
        let mut held = NoThreads {
            stop: Arc::new(AtomicBool::new(false)),
            parked: Vec::new(),
        };
        // The process already has some threads, so the cap comes before `per_process` more.
        for _ in 0..per_process {
            let stopped = Arc::clone(&held.stop);
            let spawned = std::thread::Builder::new()
                .stack_size(16 * 1024)
                .spawn(move || {
                    while !stopped.load(Ordering::SeqCst) {
                        std::thread::park();
                    }
                });
            match spawned {
                Ok(thread) => held.parked.push(thread),
                Err(e) => {
                    assert_eq!(
                        e.raw_os_error(),
                        Some(rustix::io::Errno::AGAIN.raw_os_error()),
                        "{e}"
                    );
                    return held;
                }
            }
        }
        panic!("{per_process} more threads started: the process's cap on them doesn't hold");
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

    // After the pools, so a failing assertion gives the threads back before they drop.
    let no_threads = NoThreads::new();
    assert!(
        std::thread::Builder::new().spawn(|| {}).is_err(),
        "a thread still starts"
    );

    // Every started slot busy, the next request needs another, whose thread can't start:
    // the request waits for the slot that's running, which takes it once the sleep is done.
    let waiting = busy.submit(Generation(1), echo(1));
    // Still asleep as the request came (the threads took less than its 2 s to run out), so
    // the request found the slot busy and tried to start another.
    let sleeping = sleeping
        .try_take()
        .map(|_| panic!("the sleep ended before the request came: lengthen it"))
        .unwrap_err();
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
