//! What the pool logs of a request it doesn't deliver. A superseded request that failed is
//! a worker failing, logged at warn, unless the failure was the pool's own stopping of it
//! (a kill that found its worker alive), logged at debug; one that succeeded isn't logged.
//! Each is logged in the span the request was submitted in. Alone in its process (nextest
//! runs each test in its own): it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::running::{marking_worker, submit_running};
use super::scripts;
use crate::wait::WAIT;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::kernel::KernelRequest;
use silico_core::pool::{KernelPool, Outcome};
use silico_protocol::{DebugOp, Generation};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

/// One of the pool's log events.
#[derive(Clone, Debug)]
struct Logged {
    level: Level,
    message: String,
    /// The error code it carries, if any.
    code: String,
    /// The spans it was logged in, nearest first.
    spans: Vec<&'static str>,
}

/// The pool's own log events.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<Logged>>>);

impl Events {
    /// The events logged since the last call, once one about a superseded request is among
    /// them (waiting up to [`WAIT`]): the pool logs it after delivering the outcome, and
    /// outside its lock, so it can come a moment after the outcome. Then a moment more, for
    /// any other the pool logs with it.
    fn take_once_superseded(&self) -> Vec<Logged> {
        let deadline = Instant::now() + WAIT;
        while !self
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.message.contains("superseded"))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.take_after(Duration::from_millis(50))
    }

    /// The events logged since the last call, after waiting `wait`.
    fn take_after(&self, wait: Duration) -> Vec<Logged> {
        std::thread::sleep(wait);
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Events {
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !event.metadata().target().starts_with("silico_core::pool") {
            return;
        }
        #[derive(Default)]
        struct Fields {
            message: String,
            code: String,
        }
        impl Visit for Fields {
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == "code" {
                    self.code = value.to_owned();
                }
            }
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.message = format!("{value:?}");
                }
            }
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let spans = ctx
            .event_scope(event)
            .map(|scope| scope.map(|span| span.name()).collect())
            .unwrap_or_default();
        self.0.lock().unwrap().push(Logged {
            level: *event.metadata().level(),
            message: fields.message,
            code: fields.code,
            spans,
        });
    }
}

/// The one event about a superseded request among `seen`, failing if there's not exactly
/// one: it was logged at `level`, said `text`, carried `code`, and was logged in the span
/// the request was submitted in.
fn assert_superseded_logged(seen: &[Logged], level: Level, text: &str, code: &str) {
    let about: Vec<_> = seen
        .iter()
        .filter(|e| e.message.contains("superseded"))
        .collect();
    assert_eq!(about.len(), 1, "{seen:#?}");
    let e = about[0];
    assert_eq!(e.level, level, "{e:#?}");
    assert!(e.message.contains(text), "{e:#?}");
    assert_eq!(e.code, code, "{e:#?}");
    assert!(e.spans.contains(&"recompute_feature"), "{e:#?}");
}

#[test]
fn a_superseded_failure_is_logged_at_warn_unless_the_pool_stopped_it() {
    let events = Events::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(events.clone()))
        .unwrap();
    let feature = tracing::info_span!("recompute_feature", feature = "pad1");
    let sleep_for = |millis| KernelRequest::debug(DebugOp::Sleep { millis });
    let marker = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pool-logs-received-{}", std::process::id()));
    let next = |pool: &KernelPool| Generation(pool.generation().0 + 1);

    // Stopped by the pool (its worker killed while running it): the failure is its own
    // stopping, at debug.
    let pool = KernelPool::start_on_demand(&marking_worker(&marker), 1).unwrap();
    pool.set_stop_after(Duration::ZERO);
    let stopped = submit_running(&pool, &feature, &marker, sleep_for(5_000));
    pool.advance(next(&pool));
    assert!(matches!(stopped.wait(), Outcome::Superseded));
    assert_superseded_logged(
        &events.take_once_superseded(),
        Level::DEBUG,
        "DebugSleep stopped: superseded (worker_crashed: the kernel worker crashed during \
         DebugSleep",
        "worker_crashed",
    );

    // Failed on its own after it was superseded (timed out, left running as the stop-after
    // is long): a worker failing, at warn, though not delivered.
    pool.set_stop_after(Duration::from_secs(3600));
    let timed_out = submit_running(
        &pool,
        &feature,
        &marker,
        sleep_for(5_000).with_timeout(Duration::from_secs(3)),
    );
    pool.advance(next(&pool));
    assert!(matches!(timed_out.wait(), Outcome::Superseded));
    assert_superseded_logged(
        &events.take_once_superseded(),
        Level::WARN,
        "DebugSleep failed after it was superseded: worker_timeout: the kernel worker didn't \
         finish DebugSleep within 3s",
        "worker_timeout",
    );

    // Crashed after it was superseded, before the pool came to stop it (killed from outside
    // here): a worker failing, at warn.
    let crashed = submit_running(&pool, &feature, &marker, sleep_for(5_000));
    pool.advance(next(&pool));
    let status = std::process::Command::new("kill")
        .args(["-9", &pool.pids()[0].to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(matches!(crashed.wait(), Outcome::Superseded));
    assert_superseded_logged(
        &events.take_once_superseded(),
        Level::WARN,
        "DebugSleep failed after it was superseded: worker_crashed: the kernel worker crashed \
         during DebugSleep",
        "worker_crashed",
    );
    drop(pool);

    let pool = KernelPool::start_on_demand(Path::new(TEST_WORKER), 1).unwrap();
    pool.set_stop_after(Duration::from_secs(3600));
    // Discard the pool's start event.
    events.take_after(Duration::ZERO);
    // Succeeded after it was superseded: nothing went wrong, so the pool logs nothing of it
    // (only, the first time, that its worker is running, which says nothing of the request). A
    // 1 s request superseded 0.5 s in; one not yet running by then is dropped instead (and
    // logs nothing either), so it's tried until one has run to its end on its worker.
    let mut ran = false;
    for _ in 0..10 {
        let g = pool.generation().0;
        let before = pool.pids();
        let start = Instant::now();
        let succeeded = feature.in_scope(|| pool.submit(Generation(g), sleep_for(1_000)));
        std::thread::sleep(Duration::from_millis(500));
        pool.advance(Generation(g + 1));
        assert!(matches!(succeeded.wait(), Outcome::Superseded));
        ran = start.elapsed() >= Duration::from_millis(1_000) && pool.pids() == before;
        let seen: Vec<_> = events
            .take_after(Duration::from_millis(200))
            .into_iter()
            .filter(|e| e.message != "a kernel worker is running")
            .collect();
        assert!(seen.is_empty(), "{seen:#?}");
        if ran {
            break;
        }
    }
    assert!(ran, "no superseded request ran to its end");
    drop(pool);

    // Died on its own before the pool came to stop it: the kill finds it already dead, so
    // the failure is the worker's, at warn. The worker reads the start of a request and
    // exits, leaving a process that holds its log pipe open for five seconds; the pool waits
    // up to two for the last log lines before it reports the crash. So the advance, once the
    // worker is dead and reaped, lands before its request has ended. It says it started
    // first, as a worker does, so its death is a crash (worker_crashed), not a start that
    // failed.
    let dies = scripts::script(
        "dies-holding-its-log",
        "printf '\\0\\0\\0\\0' >&3\nsleep 5 3>&- &\nhead -c 1 <&3 >/dev/null\nexit 3",
    );
    // Started now: its worker's ID is read before the request.
    let pool = KernelPool::start_workers(&dies, 1).unwrap();
    let pid = rustix::process::Pid::from_raw(i32::try_from(pool.pids()[0]).unwrap()).unwrap();
    let died = feature.in_scope(|| pool.submit(Generation(1), sleep_for(0)));
    let deadline = Instant::now() + WAIT;
    while rustix::process::test_kill_process(pid) != Err(rustix::io::Errno::SRCH) {
        assert!(Instant::now() < deadline, "the worker didn't die");
        std::thread::sleep(Duration::from_millis(5));
    }
    pool.advance(Generation(2));
    assert!(matches!(died.wait(), Outcome::Superseded));
    assert_superseded_logged(
        &events.take_once_superseded(),
        Level::WARN,
        "DebugSleep failed after it was superseded: worker_crashed: the kernel worker crashed \
         during DebugSleep",
        "worker_crashed",
    );
}
