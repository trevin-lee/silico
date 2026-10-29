//! A pool's slot whose thread panics while running a request (only a bug does: here a log
//! layer that panics once at the worker's "sent", inside the request's run) is started
//! again by the next request. The request's reply goes as its run unwinds, before the slot
//! ends, so the caller asks again while the slot is still counted started; its end then
//! starts it again for that request (#86's review). Alone in its process (nextest runs each
//! test in its own): it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::wait::WAIT;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_protocol::Generation;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

static PANICKED: AtomicBool = AtomicBool::new(false);

/// Panics at the worker's "sent" (its only trace event on a slot's thread), and, as the
/// thread unwinds, holds up the pool's event saying its slot is ending, as a loaded machine
/// can: the caller asks again meanwhile.
struct PanicInRun;

impl<S: Subscriber> Layer<S> for PanicInRun {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let on_a_slot = std::thread::current()
            .name()
            .is_some_and(|n| n.starts_with("kernel-pool-"));
        if std::thread::panicking()
            && on_a_slot
            && event.metadata().target().starts_with("silico_core::pool")
        {
            std::thread::sleep(Duration::from_millis(500));
        }
        if on_a_slot
            && event.metadata().target().starts_with("silico_core::worker")
            && *event.metadata().level() == Level::TRACE
            && !PANICKED.swap(true, Ordering::SeqCst)
        {
            panic!("a slot's thread panics running a request, as a test asks");
        }
    }
}

#[test]
fn a_slot_that_panics_running_a_request_is_started_again_by_the_next() {
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(PanicInRun))
        .unwrap();
    let pool = KernelPool::start_on_demand(
        Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker")),
        1,
    )
    .unwrap();
    let echo = |byte: u8| KernelRequest::echo(Blob::inline(vec![byte]));
    // The first request's slot panics running it: the request ends (unanswered).
    assert!(
        pool.submit(Generation(1), echo(1))
            .wait_timeout(WAIT)
            .is_ok(),
        "the first request never ended"
    );
    assert!(PANICKED.load(Ordering::SeqCst));
    // The next, asked for before the slot is back, starts it again and is answered.
    let second = pool
        .submit(Generation(1), echo(2))
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the next request waited: no slot took it"))
        .done()
        .expect("superseded")
        .unwrap();
    assert_eq!(second, Blob::inline(vec![2]));
}
