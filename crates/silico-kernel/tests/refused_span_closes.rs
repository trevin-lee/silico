//! M2.4: a request whose worker will never say it's done lets go of the span it was asked
//! in all the same: one the core refuses before sending it (too large to send), one whose
//! worker crashes, and one superseded and stopped. Alone in its process (nextest runs each
//! test in its own): it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::running::{marking_worker, submit_running};
use crate::wait::WAIT;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_protocol::{DebugOp, Generation};
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
    let pool = KernelPool::start_on_demand(
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
    wait_closed(&closed, "refused_edit");
    // A request whose worker crashes on it.
    {
        let edit = tracing::warn_span!("crashed_edit");
        let pending =
            edit.in_scope(|| pool.submit(Generation(1), KernelRequest::debug(DebugOp::Abort)));
        let err = pending
            .wait_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no outcome"))
            .done()
            .expect("superseded")
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    }
    wait_closed(&closed, "crashed_edit");
    drop(pool);
    // A request superseded while it runs, and stopped: superseded once its worker says it
    // has it, so it was sent to a running worker, not dropped from the queue before it's
    // sent.
    let marker = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("refused-span-received-{}", std::process::id()));
    let pool = KernelPool::start_on_demand(&marking_worker(&marker), 1).unwrap();
    let stop_after = Duration::from_millis(500);
    pool.set_stop_after(stop_after);
    {
        let edit = tracing::warn_span!("hung_edit");
        let sent = Instant::now();
        let pending = submit_running(
            &pool,
            &edit,
            &marker,
            // A sleep no test reaches: one left to finish has no outcome within the wait.
            KernelRequest::debug(DebugOp::Sleep { millis: 600_000 }),
        );
        pool.advance(Generation(2));
        let outcome = pending
            .wait_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no outcome"));
        assert!(outcome.done().is_none(), "not superseded");
        // It ran until it was stopped: superseded in the queue, it would end at once.
        assert!(
            sent.elapsed() >= stop_after,
            "superseded before it was sent: never stopped"
        );
    }
    wait_closed(&closed, "hung_edit");
    drop(pool);
    let _ = std::fs::remove_file(&marker);
}

fn wait_closed(closed: &Mutex<Vec<&'static str>>, name: &str) {
    let deadline = Instant::now() + WAIT;
    while !closed.lock().unwrap().contains(&name) {
        assert!(Instant::now() < deadline, "{name}'s span is still held");
        std::thread::sleep(Duration::from_millis(10));
    }
}
