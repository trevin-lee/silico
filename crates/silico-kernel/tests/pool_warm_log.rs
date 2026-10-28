//! A warm start of a pool whose worker fails at once starts each slot at most once: its
//! slots' starts counted by the span each slot's thread opens as it starts. Alone in its
//! process (nextest runs each test in its own): it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use silico_core::pool::KernelPool;
use tracing::Subscriber;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Counts the slots' threads started: each opens a `kernel_pool` span as it starts.
#[derive(Clone, Default)]
struct SlotStarts(Arc<AtomicUsize>);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for SlotStarts {
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        if attrs.metadata().name() == "kernel_pool" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn a_warm_start_of_a_worker_that_fails_at_once_starts_each_slot_at_most_once() {
    // From #263's robustness review: a worker deleted after the pool opened fails each
    // start at once, and its slot is put back unstarted while the warm start goes on;
    // counted only against the pool's size, a warm start of eight started thousands of
    // slots. Its starts are counted once: each of the eight at most once, none started.
    let starts = SlotStarts::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(starts.clone()))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("worker");
    std::fs::write(&exe, "#!/bin/sh\nexit 3\n").unwrap();
    let mut permissions = std::fs::metadata(&exe).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&exe, permissions).unwrap();
    let pool = KernelPool::start_on_demand(&exe, 8).unwrap();
    std::fs::remove_file(&exe).unwrap();
    // Again and again, as the race with a failed slot put back is won or lost by turns.
    for call in 0..30 {
        let before = starts.0.load(Ordering::SeqCst);
        assert_eq!(pool.warm(8), 0);
        let started = starts.0.load(Ordering::SeqCst) - before;
        assert!(
            started <= 8,
            "warm start {call} of eight started {started} slots"
        );
    }
}
