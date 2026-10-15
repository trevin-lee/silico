//! M2.4: a recompute's steps run in its span, under whatever started it, and name their
//! feature, so a worker's call traces back to the edit (ARCHITECTURE.md, invariant 9). Its
//! own file: it sets the process's tracing subscriber.
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
type Seen = Arc<Mutex<Vec<(Vec<&'static str>, String)>>>;

struct Steps(Seen);

struct FeatureField(String);

impl Visit for FeatureField {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "feature" {
            value.clone_into(&mut self.0);
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
        let mut feature = FeatureField(String::new());
        attrs.record(&mut feature);
        self.0.lock().unwrap().push((ancestors, feature.0));
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
    let steps = seen.lock().unwrap().clone();
    assert_eq!(steps.len(), 1, "{steps:?}");
    let (ancestors, feature) = &steps[0];
    assert_eq!(ancestors, &["recompute", "user_edit"]);
    assert_eq!(feature, "k7f2qx3m");
}
