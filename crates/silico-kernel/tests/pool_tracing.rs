//! A request through the pool runs in the tracing span it was submitted in, so its worker's
//! call (and the worker's logs, which carry that call's trace ID) trace back to the user
//! action and the feature that asked for it (ARCHITECTURE.md, invariant 9), as a call on a
//! `KernelWorker` does; and the pool's span for the request says whose it is and for which
//! of that client's generations, since each client numbers its own. Alone in its process
//! (nextest runs each test in its own): it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use silico_core::kernel::{Blob, Kernel, KernelRequest, KernelWorker};
use silico_core::pool::{KernelPool, Outcome, Pending};
use silico_protocol::Generation;
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// For every `worker_call` span, the names of its ancestors, nearest first.
struct Ancestors(Arc<Mutex<Vec<Vec<&'static str>>>>);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Ancestors {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != "worker_call" {
            return;
        }
        let chain = ctx
            .span(id)
            .map(|span| span.scope().skip(1).map(|s| s.name()).collect())
            .unwrap_or_default();
        self.0.lock().unwrap().push(chain);
    }
}

/// For every `kernel_pool_request` span, its `client` and `generation` fields, if it has
/// them.
struct PoolRequests(Arc<Mutex<Vec<Fields>>>);

/// A `kernel_pool_request` span's `client` and `generation`, if it has them.
type Fields = (Option<u64>, Option<u64>);

impl<S: Subscriber> Layer<S> for PoolRequests {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        #[derive(Default)]
        struct Seen(Option<u64>, Option<u64>);
        impl Visit for Seen {
            fn record_u64(&mut self, field: &Field, value: u64) {
                match field.name() {
                    "client" => self.0 = Some(value),
                    "generation" => self.1 = Some(value),
                    _ => {}
                }
            }
            fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
        }
        if attrs.metadata().name() != "kernel_pool_request" {
            return;
        }
        let mut fields = Seen::default();
        attrs.record(&mut fields);
        self.0.lock().unwrap().push((fields.0, fields.1));
    }
}

/// Waits for `pending`, failing the test unless it's done, and well, within 30 s.
fn done<T: std::fmt::Debug>(pending: Pending<T>) {
    match pending.wait_timeout(Duration::from_secs(30)) {
        Ok(Outcome::Done(Ok(_))) => {}
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("no outcome within 30 s"),
    }
}

#[test]
fn a_worker_call_descends_from_the_span_that_asked_for_it() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(Ancestors(Arc::clone(&seen)))
            .with(PoolRequests(Arc::clone(&requests))),
    )
    .unwrap();
    let feature = tracing::info_span!("recompute_feature", feature = "pad1");

    let mut one = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    feature
        .in_scope(|| one.run(KernelRequest::echo(Blob::inline(vec![1]))))
        .unwrap();

    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    let pending =
        feature.in_scope(|| pool.submit(Generation(1), KernelRequest::echo(Blob::inline(vec![2]))));
    done(pending);

    let chains = seen.lock().unwrap().clone();
    assert_eq!(chains.len(), 2, "{chains:?}");
    for (how, chain) in ["KernelWorker", "KernelPool"].iter().zip(&chains) {
        assert!(
            chain.contains(&"recompute_feature"),
            "{how}: worker_call's ancestors, nearest first: {chain:?}"
        );
    }
    // Through the pool, the slot it ran on is in the chain too.
    assert!(
        chains[1].contains(&"kernel_pool_request"),
        "{:?}",
        chains[1]
    );

    // The pool's own request (client 0) at its generation 1, then a client's (the first,
    // client 1) at its generation 4: each span says which.
    let client = pool.client();
    client.advance(Generation(4));
    done(client.submit(Generation(4), KernelRequest::echo(Blob::inline(vec![3]))));
    assert_eq!(
        *requests.lock().unwrap(),
        [(Some(0), Some(1)), (Some(1), Some(4))]
    );
}
