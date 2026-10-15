//! M2.4: at the default log level (warnings), where recompute's own debug spans are off, a
//! warning from the kernel during a recompute still traces back to the edit that started
//! it (ARCHITECTURE.md, invariant 9). Its own file: it sets the process's tracing subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use silico_core::document::{Checked, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Generation, Kernel, KernelRequest};
use silico_core::pool::Pending;
use silico_core::recompute::{Recompute, RunOutcome};
use tracing::Subscriber;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Each event's spans, nearest first.
type Seen = Arc<Mutex<Vec<Vec<&'static str>>>>;

struct Events(Seen);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Events {
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let scope = ctx
            .event_scope(event)
            .map(|spans| spans.map(|span| span.name()).collect())
            .unwrap_or_default();
        self.0.lock().unwrap().push(scope);
    }
}

/// Stands in for the pool: warns on each request, as it does when a worker crashes.
struct Warns(FakeKernel);

impl Kernel for Warns {
    fn generation(&self) -> Generation {
        self.0.generation()
    }

    fn advance(&self, generation: Generation) {
        self.0.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        tracing::warn!("kernel worker crashed; replacing it");
        self.0.submit(generation, request)
    }
}

#[test]
fn at_the_default_level_a_kernel_warning_still_traces_back_to_the_edit() {
    let seen: Seen = Arc::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(Events(Arc::clone(&seen)).with_filter(LevelFilter::WARN)),
    )
    .unwrap();
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let engine = Recompute::new(Warns(FakeKernel::new(1).unwrap()));
    let edit = tracing::warn_span!("user_edit");
    let run = edit.in_scope(|| engine.start(document));
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    let events = seen.lock().unwrap().clone();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        events[0].contains(&"user_edit"),
        "the warning's spans: {:?}",
        events[0]
    );
}
