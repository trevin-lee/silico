//! M2.4: at the default log level (warnings), a warning OCCT raises in a worker during a
//! recompute reaches the core's log under the step that asked for it (which names the
//! feature), the recompute, and the edit that started it: spans follow work across the
//! worker boundary (ARCHITECTURE.md, invariant 9). Alone in its process (nextest runs each
//! test in its own): it installs the process's global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use silico_core::document::{Checked, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Recompute, RunOutcome};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// A span's `feature`, if it has one.
#[derive(Clone, Default)]
struct Feature(Option<String>);

impl Visit for Feature {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "feature" {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

/// The worker's forwarded lines: each one's spans, nearest first, and the feature its step
/// names.
type Seen = Arc<Mutex<Vec<(Vec<&'static str>, Option<String>)>>>;

struct WorkerLines(Seen);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for WorkerLines {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut feature = Feature::default();
        attrs.record(&mut feature);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(feature);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != "silico::worker" {
            return;
        }
        let mut names = Vec::new();
        let mut feature = None;
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                names.push(span.name());
                if span.name() == "recompute_step" {
                    feature = span.extensions().get::<Feature>().and_then(|f| f.0.clone());
                }
            }
        }
        self.0.lock().unwrap().push((names, feature));
    }
}

#[test]
fn an_occt_warning_in_a_worker_traces_back_to_the_step_and_the_edit_with_more_work_after_it() {
    let seen: Seen = Arc::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(WorkerLines(Arc::clone(&seen)).with_filter(LevelFilter::WARN)),
    )
    .unwrap();
    // One worker, so each warning's step is followed on it by the next step (the cut on the
    // fuse), which may be asked before the warning's line is read.
    let pool =
        KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 1)
            .unwrap();
    let engine = Recompute::new(pool);
    let rounds = 200;
    for i in 0..rounds {
        // A sphere fused with a cylinder of its radius: OCCT warns about the touching
        // surfaces (only at that tangency, so it's the position that varies).
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [
            {{"id": "s0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [{i}, 0, 0], "radius": 5}},
            {{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Post", "position": [{i}, 0, 0], "radius": 5, "height": 5}},
            {{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "s0000001", "tool": "c0000001"}},
            {{"id": "b0000001", "type": "box", "v": 1, "name": "Bit", "position": [-1, -1, -20], "length": 2, "width": 2, "height": 2}},
            {{"id": "k0000001", "type": "cut", "v": 1, "name": "Cut", "base": "f0000001", "tool": "b0000001"}}
        ]}}"#
        );
        let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
        let edit = tracing::warn_span!("user_edit");
        let run = edit.in_scope(|| engine.start(document));
        assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    }
    // The worker's lines are emitted as the log sink thread gets to them.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while seen.lock().unwrap().len() < rounds {
        assert!(
            std::time::Instant::now() < deadline,
            "fewer warnings than rounds: {}",
            seen.lock().unwrap().len()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let lines = seen.lock().unwrap().clone();
    for (spans, feature) in &lines {
        assert_eq!(spans, &["recompute_step", "recompute", "user_edit"]);
        // The fuse warned, so the line names it.
        assert_eq!(feature.as_deref(), Some("f0000001"));
    }
}
