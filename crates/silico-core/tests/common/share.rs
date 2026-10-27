//! How much of a CPU this process's threads get while a test runs: what a test allows for load
//! when it bounds the wall time of work (a recompute), which [`super::lateness`] can't. A
//! machine that starves the process of CPU slows its work by about the factor it slows a
//! thread that only computes, however promptly it still wakes threads that sleep.

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The CPU a burst takes: a tenth of each cycle, so the probe costs the test a tenth of one
/// thread (taken out of what the test counts), and a burst long beside a scheduler's favour
/// for a thread that has just slept (macOS's priority decay, Linux's credit to sleepers). A
/// burst of a millisecond after each nap ran as promptly as a thread that only wakes: with 28
/// threads spinning on 14 cores and the test at nice 19, macOS ran the recompute twice as
/// slowly while such a probe saw 1.05. On CI's mutants shards (Linux, the shard's cgroup at
/// the idle CPU weight) the shard's threads run only on what other work leaves, sleeping or
/// not, and these bursts are what its baseline runs measure.
const BURST: Duration = Duration::from_millis(5);
const NAP: Duration = Duration::from_millis(45);

/// A thread that, while the probe lives, computes for [`BURST`] of its own CPU time and then
/// sleeps [`NAP`], over and over, timing each burst on the wall clock. The wall time its
/// bursts took over the CPU they got is how much slower the machine is running computing
/// threads of this process than a quiet one would: 1 on a quiet machine, 4 when the process
/// gets a quarter of a CPU per thread. Bursts are spread evenly through the run, so it's the
/// slowdown over the run, not its worst moment.
pub struct Share {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<(Duration, Duration)>,
}

/// What a [`Share`] measured.
pub struct Measured {
    /// The wall time of the probe's bursts over their CPU time, at least 1.
    pub slowdown: f64,
    /// The CPU time the probe itself took, for a test that bounds the process's CPU time.
    pub cost: Duration,
}

impl Share {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut wall = Duration::ZERO;
            let mut spent = Duration::ZERO;
            // A burst first, so even a test shorter than a nap is measured.
            loop {
                let at = Instant::now();
                let cpu = cpu_time::ThreadTime::now();
                let mut x = 0_u64;
                while cpu.elapsed() < BURST {
                    for i in 0..1000 {
                        x = black_box(x.wrapping_mul(31).wrapping_add(i));
                    }
                }
                black_box(x);
                wall += at.elapsed();
                spent += cpu.elapsed();
                if stopping.load(Ordering::Relaxed) {
                    return (wall, spent);
                }
                std::thread::sleep(NAP);
            }
        });
        Self { stop, thread }
    }

    /// What the probe measured, from its start until now.
    pub fn finish(self) -> Measured {
        self.stop.store(true, Ordering::Relaxed);
        let (wall, spent) = self.thread.join().unwrap();
        // `spent` is at least one burst, so never zero.
        let slowdown = (wall.as_secs_f64() / spent.as_secs_f64()).max(1.0);
        Measured {
            slowdown,
            cost: spent,
        }
    }
}
