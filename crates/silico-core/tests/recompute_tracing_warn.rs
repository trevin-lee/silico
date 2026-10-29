//! M2.4: at the default log level (warnings), a warning raised on a step's thread during a
//! recompute is under its step's span, naming the feature, its recompute's, naming the
//! generation, and the edit that started it (ARCHITECTURE.md, invariant 9). Alone in its
//! process (nextest runs each test in its own), it sets the process's tracing subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::example;

use std::sync::{Arc, Mutex};

use example::EXAMPLE;
use silico_core::document::{Checked, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Generation, Kernel, KernelRequest};
use silico_core::pool::Pending;
use silico_core::recompute::{Recompute, RunOutcome};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// A span's `feature` and `generation`, if it has them.
#[derive(Clone, Debug, Default, PartialEq)]
struct Fields {
    feature: Option<String>,
    generation: Option<u64>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "feature" {
            self.feature = Some(value.to_owned());
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "generation" {
            self.generation = Some(value);
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

/// Each event's spans, nearest first, with their fields.
type Seen = Arc<Mutex<Vec<Vec<(&'static str, Fields)>>>>;

struct Events(Seen);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Events {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let scope = ctx
            .event_scope(event)
            .map(|spans| {
                spans
                    .map(|span| {
                        let fields = span
                            .extensions()
                            .get::<Fields>()
                            .cloned()
                            .unwrap_or_default();
                        (span.name(), fields)
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.0.lock().unwrap().push(scope);
    }
}

/// A synthetic warning, raised on the step's thread as each step is asked of the kernel:
/// what any warning there (the pool's of a request it doesn't deliver, say) is under.
struct Warns(FakeKernel);

impl Kernel for Warns {
    fn identity(&self) -> &str {
        self.0.identity()
    }

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
        tracing::warn!("a warning during the step");
        self.0.submit(generation, request)
    }
}

#[test]
fn at_the_default_level_a_warning_in_a_step_names_its_feature_generation_and_edit() {
    let seen: Seen = Arc::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(Events(Arc::clone(&seen)).with_filter(LevelFilter::WARN)),
    )
    .unwrap();
    // The design's example: the block made, the pin made and moved, the cut: four steps,
    // and three features' solids measured (the block's, the pin's move's, the cut's), each
    // in two requests (its summary, and its elements described in one), each request
    // warning once: 4 + 3 × 2.
    let document = Checked::new(read(EXAMPLE.as_bytes()).unwrap()).unwrap();
    let engine = Recompute::new(Warns(FakeKernel::new(2).unwrap()));
    let edit = tracing::warn_span!("user_edit");
    let run = edit.in_scope(|| engine.start(document));
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    let events = seen.lock().unwrap().clone();
    assert_eq!(events.len(), 10, "{events:?}");
    let mut features = Vec::new();
    for scope in &events {
        let names: Vec<&str> = scope.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["recompute_step", "recompute", "user_edit"]);
        features.push(scope[0].1.feature.clone().unwrap());
        // The first recompute on a new kernel is generation 1.
        assert_eq!(scope[1].1.generation, Some(1));
    }
    features.sort_unstable();
    // The pin's make and move are the pin's; the block's make, the block's; the cut, the
    // hole's; and each solid's measure its feature's.
    let each = |id: &'static str, n: usize| std::iter::repeat_n(id, n);
    let expected: Vec<&str> = each("a91cw0de", 4)
        .chain(each("k7f2qx3m", 3))
        .chain(each("m3tq8v0z", 3))
        .collect();
    assert_eq!(features, expected);
}
