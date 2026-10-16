//! M2.4: a request the core refuses before sending it (too large to send) lets go of the
//! span it was asked in at once: the worker will never say it's done. Alone in its binary:
//! it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_protocol::Generation;
use tracing::Subscriber;
use tracing::span::Id;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

struct Closed(Arc<Mutex<Vec<&'static str>>>);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Closed {
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            self.0.lock().unwrap().push(span.name());
        }
    }
}

#[test]
fn a_request_refused_before_it_was_sent_lets_its_span_go() {
    let closed = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Closed(Arc::clone(&closed))),
    )
    .unwrap();
    let pool = KernelPool::start_workers(
        Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker")),
        1,
    )
    .unwrap();
    {
        let edit = tracing::warn_span!("refused_edit");
        let too_large = vec![0; silico_protocol::MAX_FRAME_LEN as usize + 1];
        let pending = edit
            .in_scope(|| pool.submit(Generation(1), KernelRequest::echo(Blob::inline(too_large))));
        let err = pending
            .wait_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no outcome"))
            .done()
            .expect("superseded")
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    }
    // The pool stays, and asks its worker nothing more: the span closes all the same.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !closed.lock().unwrap().contains(&"refused_edit") {
        assert!(
            Instant::now() < deadline,
            "the refused request's span is still held"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(pool);
}
