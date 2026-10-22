//! For the pool's back-off tests: workers that log each launch, the times between them,
//! and what the pool's slots decide each time a worker ends.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// How long a test here waits for a launch, a request, or a full pool before failing: long
/// enough that only a hang reaches it, however loaded the machine. A hang guard only: what
/// the tests measure comes from the times their workers log, or from bounds that what they
/// rule out can't meet.
pub const HANG: Duration = Duration::from_secs(60);

/// A worker script that logs the time of each launch to a file (seconds since the epoch),
/// then runs `then`.
pub fn launch_logger(dir: &Path, then: &str) -> (PathBuf, PathBuf) {
    let script = dir.join("worker.sh");
    let log = dir.join("launches");
    std::fs::write(
        &script,
        format!("#!/bin/sh\n{}\n{then}\n", log_launch(&log)),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

/// A shell line that logs the time to `log`, in seconds since the epoch.
pub fn log_launch(log: &Path) -> String {
    format!(
        "echo \"$(perl -MTime::HiRes=time -e 'print time') $$\" >> '{}'",
        log.display()
    )
}

/// The launches logged to `log` so far, earliest first: when, and the process ID.
pub fn launches(log: &Path) -> Vec<(f64, u32)> {
    let mut launches: Vec<(f64, u32)> = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let (time, pid) = l.trim().split_once(' ').unwrap();
            (time.parse().unwrap(), pid.parse().unwrap())
        })
        .collect();
    launches.sort_by(|a, b| a.0.total_cmp(&b.0));
    launches
}

/// The launch times logged to `log` so far, earliest first.
pub fn launch_times(log: &Path) -> Vec<f64> {
    launches(log).into_iter().map(|(time, _)| time).collect()
}

/// Waits for `log` to hold `n` launches logged after `after`, and gives them, earliest
/// first; failing only after [`HANG`].
pub fn wait_launches_after(log: &Path, after: f64, n: usize) -> Vec<f64> {
    let deadline = Instant::now() + HANG;
    loop {
        let times: Vec<f64> = launch_times(log)
            .into_iter()
            .filter(|&t| t > after)
            .collect();
        if times.len() >= n {
            return times;
        }
        assert!(
            Instant::now() < deadline,
            "{} launches after {HANG:?}, not {n}: at {times:?}",
            times.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits for `log` to hold `n` launches, and gives them, earliest first.
pub fn wait_launches(log: &Path, n: usize) -> Vec<f64> {
    wait_launches_after(log, f64::NEG_INFINITY, n)
}

/// The gaps between `times`.
pub fn gaps(times: &[f64]) -> Vec<f64> {
    times.windows(2).map(|w| w[1] - w[0]).collect()
}

/// Checks that each gap between `times` is longer than the wait `waits` gives it. A slot
/// waits from the idle check that sees its worker's end, which comes after the worker has
/// logged its launch: so the gap from that launch to the next is longer than the wait,
/// however late a loaded machine runs the checks.
pub fn assert_gaps_exceed(times: &[f64], waits: &[f64]) {
    let gaps = gaps(times);
    assert!(gaps.len() >= waits.len(), "launches at {times:?}");
    for (k, (gap, wait)) in gaps.iter().zip(waits).enumerate() {
        assert!(
            gap > wait,
            "gap {k} is {gap:.3} s, not longer than its {wait} s wait: gaps {gaps:?}"
        );
    }
}

/// What a slot decided once its worker ended, or while it waited, as the pool says it in its
/// span's events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// To start another worker at once.
    AtOnce,
    /// To start another after this many milliseconds.
    Wait(u64),
    /// To start another before its wait was over: a worker started since it began.
    WaitEnded,
}

/// Every decision the pool's slots say, in order, with each one's slot; and, apart, each
/// slot's note that its worker is running (not a decision: what makes a kill of it one from
/// outside).
#[derive(Clone, Default)]
pub struct Decisions(Arc<Mutex<Vec<(u64, Decision)>>>, Arc<Mutex<Vec<u64>>>);

impl<S: Subscriber> Layer<S> for Decisions {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        #[derive(Default)]
        struct Fields {
            message: String,
            slot: Option<u64>,
            wait_ms: Option<u64>,
        }
        impl Visit for Fields {
            fn record_u64(&mut self, field: &Field, value: u64) {
                match field.name() {
                    "slot" => self.slot = Some(value),
                    "wait_ms" => self.wait_ms = Some(value),
                    _ => {}
                }
            }
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.message = format!("{value:?}");
                }
            }
        }
        if !event.metadata().target().starts_with("silico_core::pool") {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        if fields.message == "a kernel worker is running"
            && let Some(slot) = fields.slot
        {
            self.1.lock().unwrap().push(slot);
            return;
        }
        let decision = match fields.wait_ms {
            Some(ms) => Decision::Wait(ms),
            None if fields.message.contains("starting another at once") => Decision::AtOnce,
            None if fields.message.contains("starting another now") => Decision::WaitEnded,
            None => return,
        };
        let Some(slot) = fields.slot else { return };
        self.0.lock().unwrap().push((slot, decision));
    }
}

impl Decisions {
    /// Installs the subscriber that records them, for this test's process.
    pub fn record() -> Decisions {
        let decisions = Decisions::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(decisions.clone()),
        )
        .expect("one test a process, as nextest runs them");
        decisions
    }

    /// How many of `slot`'s workers it has noted running.
    pub fn running(&self, slot: u64) -> usize {
        self.1
            .lock()
            .unwrap()
            .iter()
            .filter(|&&s| s == slot)
            .count()
    }

    /// The first `n` decisions of `slot`, in order.
    pub fn of_slot(&self, slot: u64, n: usize) -> Vec<Decision> {
        let decisions = self.0.lock().unwrap();
        decisions
            .iter()
            .filter(|(s, _)| *s == slot)
            .map(|(_, d)| *d)
            .take(n)
            .collect()
    }
}
