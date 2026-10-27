//! A pool's slot whose thread ends unexpectedly while running a request (a panic, which
//! only a bug causes: here a log layer that panics once as a request's span opens) leaves
//! no entry behind: no worker listed for the dead slot, and the next request starts it
//! again and is answered. Alone in its process (nextest runs each test in its own): it
//! installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::wait::WAIT;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_protocol::Generation;
use tracing::Subscriber;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

static PANICKED: AtomicBool = AtomicBool::new(false);

/// Panics as the first request's span opens, its worker started and the slot running it.
struct PanicOnce;

impl<S: Subscriber> Layer<S> for PanicOnce {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        if attrs.metadata().name() == "kernel_pool_request"
            && !PANICKED.swap(true, Ordering::SeqCst)
        {
            panic!("a slot's thread ends while running, as a test asks");
        }
    }
}

#[test]
fn a_slot_whose_thread_ends_while_running_leaves_no_entry_behind() {
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(PanicOnce))
        .unwrap();
    let pool = KernelPool::start_on_demand(
        Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker")),
        1,
    )
    .unwrap();
    let echo = |byte: u8| KernelRequest::echo(Blob::inline(vec![byte]));
    assert!(
        pool.submit(Generation(1), echo(1))
            .wait_timeout(WAIT)
            .is_ok(),
        "the first request never ended"
    );
    assert!(PANICKED.load(Ordering::SeqCst));
    let deadline = Instant::now() + WAIT;
    while !pool.pids().is_empty() {
        assert!(
            Instant::now() < deadline,
            "a dead slot still lists a worker: {:?}",
            pool.pids()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let second = pool
        .submit(Generation(1), echo(2))
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the next request waited: no slot took it"))
        .done()
        .expect("superseded")
        .unwrap();
    assert_eq!(second, Blob::inline(vec![2]));
    assert_eq!(pool.pids().len(), 1);
}
