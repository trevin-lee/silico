//! A pool's slot whose thread panics every time it starts its worker (only a bug does: here
//! a log layer that panics at each slot thread's first worker event) starts its worker once
//! per request waiting, never in a loop of its own: each start takes a request, which ends
//! with it, and with none waiting no worker is started. (A slot started again with no
//! request would start no worker, so this counts worker starts, not threads.) Alone in its
//! process (nextest runs each test in its own): it installs the process's global
//! subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::wait::WAIT;
use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_protocol::Generation;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// How many slot threads have panicked: one per start.
static PANICS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static PANICKED_HERE: Cell<bool> = const { Cell::new(false) };
}

/// Panics at each slot thread's first worker event (its worker's start).
struct PanicEveryStart;

impl<S: Subscriber> Layer<S> for PanicEveryStart {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let on_a_slot = std::thread::current()
            .name()
            .is_some_and(|n| n.starts_with("kernel-pool-"));
        if on_a_slot
            && !std::thread::panicking()
            && event.metadata().target().starts_with("silico_core::worker")
            && !PANICKED_HERE.with(|p| p.replace(true))
        {
            PANICS.fetch_add(1, Ordering::SeqCst);
            panic!("a slot's thread panics as it starts its worker, as a test asks");
        }
    }
}

#[test]
fn a_slot_that_panics_at_every_start_starts_once_per_request_and_not_on_its_own() {
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(PanicEveryStart))
        .unwrap();
    let pool = KernelPool::start_on_demand(
        Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker")),
        1,
    )
    .unwrap();
    let echo = |byte: u8| KernelRequest::echo(Blob::inline(vec![byte]));
    // Eight requests at once, for one slot: each start takes one, and ends with it.
    let pending: Vec<_> = (0..8)
        .map(|i| pool.submit(Generation(1), echo(i)))
        .collect();
    for (i, p) in pending.into_iter().enumerate() {
        assert!(p.wait_timeout(WAIT).is_ok(), "request {i} never ended");
    }
    assert_eq!(PANICS.load(Ordering::SeqCst), 8, "one start per request");
    // With none waiting, no worker is started again.
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        PANICS.load(Ordering::SeqCst),
        8,
        "the slot started on its own"
    );
    assert!(pool.pids().is_empty(), "{:?}", pool.pids());
    // The next request starts it once more.
    assert!(
        pool.submit(Generation(1), echo(9))
            .wait_timeout(WAIT)
            .is_ok()
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(PANICS.load(Ordering::SeqCst), 9);
}
