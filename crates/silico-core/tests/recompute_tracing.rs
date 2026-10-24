//! M2.4: a recompute's steps run in its span, under whatever started it, and name their
//! feature, so a worker's call traces back to the edit (ARCHITECTURE.md, invariant 9).
//! Alone in its process (nextest runs each test in its own), it sets the process's tracing
//! subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use silico_core::document::{Checked, read};
use silico_core::fake::FakeKernel;
use silico_core::recompute::{Recompute, RunOutcome};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Each `recompute_step` span: its ancestors' names, nearest first, and its `feature`.
type Seen = Arc<Mutex<Vec<(Vec<&'static str>, String, String)>>>;

struct Steps(Seen);

/// A step span's `feature` and `job`.
#[derive(Default)]
struct FeatureField(String, String);

impl Visit for FeatureField {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "feature" => value.clone_into(&mut self.0),
            "job" => value.clone_into(&mut self.1),
            _ => {}
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Steps {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != "recompute_step" {
            return;
        }
        let ancestors = ctx
            .span(id)
            .map(|span| span.scope().skip(1).map(|s| s.name()).collect())
            .unwrap_or_default();
        let mut fields = FeatureField::default();
        attrs.record(&mut fields);
        self.0.lock().unwrap().push((ancestors, fields.0, fields.1));
    }
}

#[test]
fn a_recompute_step_descends_from_its_recompute_and_the_edit_that_asked_and_names_its_feature() {
    let seen: Seen = Arc::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Steps(Arc::clone(&seen))),
    )
    .unwrap();
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let engine = Recompute::new(FakeKernel::new(1).unwrap());
    let edit = tracing::info_span!("user_edit");
    let run = edit.in_scope(|| engine.start(document));
    assert!(matches!(run.wait().unwrap(), RunOutcome::Published(_)));
    let mut steps = seen.lock().unwrap().clone();
    steps.sort();
    // The box's make and the measure of its solid, each under the recompute and the edit,
    // each naming the feature.
    let under = || vec!["recompute", "user_edit"];
    assert_eq!(
        steps,
        [
            (under(), "k7f2qx3m".to_owned(), "make".to_owned()),
            (under(), "k7f2qx3m".to_owned(), "measure".to_owned()),
        ]
    );
}
