//! How much slower than a quiet machine this process runs work that hands itself from thread to
//! thread: what a test allows for load when it bounds the wall time of such work (a
//! recompute, whose steps wake one thread after another), which [`super::lateness`] can't. A
//! loaded machine slows that work twice over: a thread computes slower when it gets less of a
//! CPU, and each thread woken to go on waits for one. A probe that only computes misses the
//! waits, and one that only sleeps misses both.
// Shared with the render tests, which read only the slowdown.
#![allow(dead_code)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The CPU each hand of a relay computes before it hands the relay on: about what a
/// recompute's step costs between its wakes on a quiet machine (a chain of four hundred
/// booleans took 0.46 s for some two thousand wakes), so a probe's hand is scheduled as a
/// step is. A probe that computed a millisecond after each nap ran as promptly as a thread
/// that only wakes: with 28 threads spinning on 14 cores and the test at nice 19, macOS ran
/// the recompute twice as slowly while it saw 1.05; and one that computes without waking
/// misses the wakes, which on a loaded CI shard (an idle-weight cgroup) cost the chain some
/// 6 ms a feature while its worst wake ran under 25 ms late (run 40488004644).
const HAND: Duration = Duration::from_micros(200);
/// Hands each way in a relay, and the nap between relays: a relay's CPU is 4 ms of every 40,
/// so the probe costs the test about a tenth of a thread (taken out of what the test counts).
const HANDS: usize = 10;
const NAP: Duration = Duration::from_millis(36);

/// Two threads that, while the probe lives, run a relay and then nap, over and over: in a
/// relay each in turn computes [`HAND`] of its own CPU time and hands a token to the other,
/// [`HANDS`] times each way, as a recompute's threads hand work on. The wall time of the
/// relays over the CPU their hands took is how much slower the machine runs such work than a
/// quiet one: a little over 1 on a quiet machine (each hand-off costs a wake), and as much as
/// the machine delays wakes and starves threads under load. Relays are spread evenly through
/// the run, so it's the slowdown over the run, not its worst moment.
pub struct Share {
    stop: Arc<AtomicBool>,
    driver: JoinHandle<(Duration, Duration)>,
    echo: JoinHandle<Duration>,
}

/// What a [`Share`] measured.
pub struct Measured {
    /// The wall time of the probe's relays over the CPU their hands took, at least 1.
    pub slowdown: f64,
    /// The CPU time the probe itself took, for a test that bounds the process's CPU time.
    pub cost: Duration,
}

/// Computes for `hand` of this thread's CPU time.
fn compute(hand: Duration) {
    let cpu = cpu_time::ThreadTime::now();
    let mut x = 0_u64;
    while cpu.elapsed() < hand {
        for i in 0..100 {
            x = black_box(x.wrapping_mul(31).wrapping_add(i));
        }
    }
    black_box(x);
}

impl Share {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let (to_echo, from_driver) = sync_channel::<()>(1);
        let (to_driver, from_echo) = sync_channel::<()>(1);
        // The other hand: computes and hands back each token until the driver hangs up. All
        // its CPU is the relays', since between them it only blocks.
        let echo = std::thread::spawn(move || {
            let cpu = cpu_time::ThreadTime::now();
            while from_driver.recv().is_ok() {
                compute(HAND);
                if to_driver.send(()).is_err() {
                    break;
                }
            }
            cpu.elapsed()
        });
        let driver = std::thread::spawn(move || {
            let mut wall = Duration::ZERO;
            let mut spent = Duration::ZERO;
            // A relay first, so even a test shorter than a nap is measured.
            loop {
                let at = Instant::now();
                let cpu = cpu_time::ThreadTime::now();
                for _ in 0..HANDS {
                    compute(HAND);
                    to_echo.send(()).unwrap();
                    from_echo.recv().unwrap();
                }
                wall += at.elapsed();
                spent += cpu.elapsed();
                if stopping.load(Ordering::Relaxed) {
                    // Returning drops `to_echo`, which ends the echo.
                    return (wall, spent);
                }
                std::thread::sleep(NAP);
            }
        });
        Self { stop, driver, echo }
    }

    /// What the probe measured, from its start until now.
    pub fn finish(self) -> Measured {
        self.stop.store(true, Ordering::Relaxed);
        let (wall, driven) = self.driver.join().unwrap();
        let echoed = self.echo.join().unwrap();
        let spent = driven + echoed;
        // At least one relay's hands, so never zero.
        let slowdown = (wall.as_secs_f64() / spent.as_secs_f64()).max(1.0);
        Measured {
            slowdown,
            cost: spent,
        }
    }
}
