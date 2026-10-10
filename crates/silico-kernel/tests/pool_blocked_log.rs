//! A log that blocks (a client's stderr that no one reads) doesn't hold up the pool: the
//! pool logs a superseded request's failure outside its lock, in the span the request was
//! submitted in. Alone in its binary: it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::kernel::KernelRequest;
use silico_core::pool::{KernelPool, Outcome};
use silico_protocol::{DebugOp, Generation};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

/// Blocks on the pool's event about a superseded request until released (or ten seconds
/// pass), and records the spans it was logged in.
#[derive(Clone, Default)]
struct Blocking {
    entered: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    spans: Arc<Mutex<Vec<Vec<&'static str>>>>,
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Blocking {
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !event.metadata().target().starts_with("silico_core::pool") {
            return;
        }
        struct Message(String);
        impl Visit for Message {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        if !message.0.contains("superseded") {
            return;
        }
        let spans = ctx
            .event_scope(event)
            .map(|scope| scope.map(|span| span.name()).collect())
            .unwrap_or_default();
        self.spans.lock().unwrap().push(spans);
        self.entered.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.release.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn a_blocked_superseded_log_neither_holds_the_pool_nor_loses_its_span() {
    let layer = Blocking::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer.clone()))
        .unwrap();
    let pool = KernelPool::start_workers(Path::new(TEST_WORKER), 1).unwrap();
    pool.set_stop_after(Duration::ZERO);
    let feature = tracing::info_span!("recompute_feature", feature = "pad1");
    let stopped = feature.in_scope(|| {
        pool.submit(
            Generation(1),
            KernelRequest::debug(DebugOp::Sleep { millis: 5_000 }),
        )
    });
    std::thread::sleep(Duration::from_millis(100));
    pool.advance(Generation(2));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !layer.entered.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the stopped request wasn't logged"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // While the log blocks, the pool's lock is free: another thread reads the generation.
    let (tx, rx) = std::sync::mpsc::channel();
    let free = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = tx.send(pool.generation());
        });
        let free = rx.recv_timeout(Duration::from_secs(1)).is_ok();
        layer.release.store(true, Ordering::SeqCst);
        free
    });
    assert!(matches!(stopped.wait(), Outcome::Superseded));
    assert!(free, "a blocked log held the pool's lock");
    let spans = layer.spans.lock().unwrap().clone();
    assert!(
        !spans.is_empty() && spans.iter().all(|s| s.contains(&"recompute_feature")),
        "the superseded request's log isn't in its submitter's span: {spans:?}"
    );
}
