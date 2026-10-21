//! A pool's slot whose thread ends unexpectedly (a panic, which only a bug causes: here a
//! log layer that panics once on a slot's thread) is started again by the next request,
//! rather than counted free with no thread to take it. Alone in its binary: it installs
//! the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_protocol::Generation;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

static PANICKED: AtomicBool = AtomicBool::new(false);

/// Panics at the first worker event on a slot's thread (its worker's start).
struct PanicOnce;

impl<S: Subscriber> Layer<S> for PanicOnce {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let on_a_slot = std::thread::current()
            .name()
            .is_some_and(|n| n.starts_with("kernel-pool-"));
        if event.metadata().target().starts_with("silico_core::worker")
            && on_a_slot
            && !PANICKED.swap(true, Ordering::SeqCst)
        {
            panic!("a slot's thread ends unexpectedly, as a test asks");
        }
    }
}

#[test]
fn a_slot_whose_thread_ends_unexpectedly_is_started_again() {
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(PanicOnce))
        .unwrap();
    let pool = KernelPool::start_on_demand(
        Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker")),
        1,
    )
    .unwrap();
    let echo = |byte: u8| KernelRequest::echo(Blob::inline(vec![byte]));
    // The first request's slot panics starting its worker: the request ends (unanswered).
    assert!(
        pool.submit(Generation(1), echo(1))
            .wait_timeout(Duration::from_secs(10))
            .is_ok(),
        "the first request never ended"
    );
    assert!(PANICKED.load(Ordering::SeqCst));
    // The next starts the slot again, and is answered.
    let second = pool
        .submit(Generation(1), echo(2))
        .wait_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| panic!("the next request waited: no slot took it"))
        .done()
        .expect("superseded")
        .unwrap();
    assert_eq!(second, Blob::inline(vec![2]));
}
