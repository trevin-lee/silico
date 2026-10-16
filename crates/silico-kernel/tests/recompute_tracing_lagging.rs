//! M2.4: a log sink that's slow (a stalled terminal, a busy exporter) puts the core's log
//! thread behind a worker, and the worker's lines still go under the spans of the requests
//! they're for, however far behind. The sink here waits at the first worker line until
//! every recompute has run, so the log thread is certainly behind by all of them, on any
//! machine. Alone in its binary: it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use silico_core::document::{Checked, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Recompute, RunOutcome};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// A span's `round`, if it has one.
#[derive(Clone, Copy, Default)]
struct Round(Option<u64>);

impl Visit for Round {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "round" {
            self.0 = Some(value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "round" {
            self.0 = u64::try_from(value).ok();
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

/// Each worker line's spans, nearest first, and its edit's round.
type Seen = Arc<Mutex<Vec<(Vec<&'static str>, Option<u64>)>>>;

/// Opens when every round has run.
type Gate = Arc<(Mutex<bool>, Condvar)>;

struct BlockedWorkerLines {
    seen: Seen,
    gate: Gate,
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for BlockedWorkerLines {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut round = Round::default();
        attrs.record(&mut round);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(round);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != "silico::worker" {
            return;
        }
        // The sink is stalled until every round has run.
        let (open, opened) = &*self.gate;
        let mut open = open.lock().unwrap();
        while !*open {
            open = opened.wait(open).unwrap();
        }
        drop(open);
        let mut names = Vec::new();
        let mut round = None;
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                names.push(span.name());
                if span.name() == "user_edit" {
                    round = span.extensions().get::<Round>().and_then(|r| r.0);
                }
            }
        }
        self.seen.lock().unwrap().push((names, round));
    }
}

#[test]
fn a_worker_line_read_long_after_its_request_still_goes_under_its_span() {
    let seen: Seen = Arc::default();
    let gate: Gate = Arc::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(
            BlockedWorkerLines {
                seen: Arc::clone(&seen),
                gate: Arc::clone(&gate),
            }
            .with_filter(LevelFilter::WARN),
        ),
    )
    .unwrap();
    let pool = KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 1)
        .unwrap();
    let engine = Recompute::new(pool);
    // Few enough that the worker's lines (a warning and three done lines a round) fit in its
    // log pipe while the sink is stalled; many more than a small bound on the requests the
    // core remembers would hold.
    let rounds: u64 = 20;
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
        let edit = tracing::warn_span!("user_edit", round = i);
        let run = edit.in_scope(|| engine.start(document));
        assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    }
    // Every round has run: the sink goes on, its log thread every round behind.
    let (open, opened) = &*gate;
    *open.lock().unwrap() = true;
    opened.notify_all();
    let deadline = Instant::now() + Duration::from_secs(60);
    while (seen.lock().unwrap().len() as u64) < rounds {
        assert!(Instant::now() < deadline, "fewer warnings than rounds");
        std::thread::sleep(Duration::from_millis(10));
    }
    let lines = seen.lock().unwrap().clone();
    for (k, (spans, round)) in lines.iter().enumerate() {
        assert_eq!(
            spans,
            &["recompute_step", "recompute", "user_edit"],
            "line {k}"
        );
        // Each line under its own round's edit, not another's.
        assert_eq!(*round, Some(k as u64), "line {k}");
    }
}
