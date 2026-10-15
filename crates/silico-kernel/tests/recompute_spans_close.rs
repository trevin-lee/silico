//! M2.4: a recompute's spans, and the edit's that started it, close once it's done: a
//! worker lets the span of a request go when it has read the request's last log line, not
//! at its next request. Alone in its binary: it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::document::{Checked, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Recompute, RunOutcome};
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
fn the_recompute_and_edit_spans_close_once_the_recompute_is_done() {
    let closed = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Closed(Arc::clone(&closed))),
    )
    .unwrap();
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "s0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let pool = KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 1)
        .unwrap();
    // The engine (and its worker) stays: the spans must close without a next request.
    let engine = Recompute::new(pool);
    {
        let edit = tracing::warn_span!("user_edit");
        let run = edit.in_scope(|| engine.start(document));
        assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let names = closed.lock().unwrap().clone();
        if ["recompute_step", "recompute", "user_edit"]
            .iter()
            .all(|name| names.contains(name))
        {
            break;
        }
        assert!(Instant::now() < deadline, "still open: {names:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(engine);
}
