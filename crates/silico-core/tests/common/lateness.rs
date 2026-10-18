//! How late this machine runs threads that wake from a sleep: what a test allows for load
//! when it bounds how soon something that only waits must happen. Not how much CPU a
//! process gets: a loaded machine can wake a sleeping thread within milliseconds while work
//! that computes (a worker starting, a recompute) runs several times slower, so this bounds
//! a wake, a kill, a reap, a delivery, never work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A thread that sleeps 50 ms over and over while the probe lives, noting the most any sleep
/// overran: how late the machine is waking threads meanwhile. On a quiet machine a few
/// milliseconds; on a loaded one, as much as load delays a wake (macOS favours threads that
/// sleep, so often little even then), so a bound built on it grows with how late wakes run.
pub struct Lateness {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Duration>,
}

impl Lateness {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let nap = Duration::from_millis(50);
            let mut most = Duration::ZERO;
            while !stopping.load(Ordering::Relaxed) {
                let start = Instant::now();
                std::thread::sleep(nap);
                most = most.max(start.elapsed().saturating_sub(nap));
            }
            most
        });
        Self { stop, thread }
    }

    /// The most a sleep overran while the probe ran (including the one it was in).
    pub fn most(self) -> Duration {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap()
    }
}

/// How late a thing due at a moment may come, given `overran` (a [`Lateness`]): `base`, what
/// it takes on a quiet machine with room to spare, and ten times the machine's lateness, for
/// the few threads that must wake in turn before it's seen (a thread woken to act, another
/// that sees what it did, the test's own).
pub fn allowance(base: Duration, overran: Duration) -> Duration {
    base + overran * 10
}
