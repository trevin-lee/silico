//! M2.4: a step whose input failed in an earlier recompute, its failure kept, isn't started
//! at all: counted by its span. Its own file: it sets the process's tracing subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Op, Primitive};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use tracing::Subscriber;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Counts the recompute steps started.
struct Steps(Arc<Mutex<usize>>);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Steps {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        if attrs.metadata().name() == "recompute_step" {
            *self.0.lock().unwrap() += 1;
        }
    }
}

fn published(engine: &Recompute<FakeKernel>, text: &str) -> Arc<Model> {
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

#[test]
fn a_step_on_a_kept_failure_isnt_started() {
    let steps = Arc::new(Mutex::new(0));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Steps(Arc::clone(&steps))),
    )
    .unwrap();
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
    });
    // A moved box: its make, refused and kept, and its move, which never runs.
    let moved = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [1, 0, 0], "length": 1, "width": 1, "height": 1}]}"#;
    let model = published(&engine, moved);
    let id = FeatureId::new("aaaaaaaa").unwrap();
    assert!(matches!(
        model.outcome(&id),
        Some(FeatureOutcome::Failed(_))
    ));
    assert_eq!(*steps.lock().unwrap(), 1, "only the make was started");
    // Again: the make's refusal is kept, and the move, on it, isn't started either.
    published(&engine, moved);
    assert_eq!(
        *steps.lock().unwrap(),
        1,
        "a step on a kept failure was started"
    );
}
