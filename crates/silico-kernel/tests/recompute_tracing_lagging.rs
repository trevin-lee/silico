//! M2.4: a log sink that's slow (a stalled terminal, a busy exporter) puts the core's log
//! thread behind a worker, and the worker's lines still go under the spans of the requests
//! they're for, however far behind. Alone in its binary: it installs the process's global
//! subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silico_core::document::{Checked, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Recompute, RunOutcome};
use tracing::Subscriber;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Each worker line's spans, nearest first, recorded slowly.
type Seen = Arc<Mutex<Vec<Vec<&'static str>>>>;

struct SlowWorkerLines(Seen);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for SlowWorkerLines {
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != "silico::worker" {
            return;
        }
        let names = ctx
            .event_scope(event)
            .map(|spans| spans.map(|span| span.name()).collect())
            .unwrap_or_default();
        self.0.lock().unwrap().push(names);
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_worker_line_read_long_after_its_request_still_goes_under_its_span() {
    let seen: Seen = Arc::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(SlowWorkerLines(Arc::clone(&seen)).with_filter(LevelFilter::WARN)),
    )
    .unwrap();
    let pool = KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 1)
        .unwrap();
    let engine = Recompute::new(pool);
    let rounds = 30;
    for i in 0..rounds {
        // A sphere fused with a cylinder of its radius: OCCT warns about the touching
        // surfaces.
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [
            {{"id": "s0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [{i}, 0, 0], "radius": 5}},
            {{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Post", "position": [{i}, 0, 0], "radius": 5, "height": 5}},
            {{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "s0000001", "tool": "c0000001"}}
        ]}}"#
        );
        let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
        let edit = tracing::warn_span!("user_edit");
        let run = edit.in_scope(|| engine.start(document));
        assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    while seen.lock().unwrap().len() < rounds {
        assert!(Instant::now() < deadline, "fewer warnings than rounds");
        std::thread::sleep(Duration::from_millis(50));
    }
    let lines = seen.lock().unwrap().clone();
    let orphans: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, spans)| spans.as_slice() != ["recompute_step", "recompute", "user_edit"])
        .map(|(i, _)| i)
        .collect();
    assert!(orphans.is_empty(), "lines without their spans: {orphans:?}");
}
