//! M2.4: a step whose input failed in an earlier recompute, its failure kept, isn't started
//! at all: counted by its span. Alone in its process (nextest runs each test in its own),
//! it sets the process's tracing subscriber.
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

/// Spins until `gap` has passed on the test's own monotonic clock: time let pass, never
/// slept.
fn spin(gap: std::time::Duration) {
    let from = std::time::Instant::now();
    while from.elapsed() < gap {
        std::thread::yield_now();
    }
}

#[test]
fn a_superseded_or_cancelled_runs_clock_ends_as_it_stops_though_its_step_is_still_held() {
    // A run held at its box past its supersede (as the pool lets a running request go on
    // for its grace): superseded by a newer run, then, another time, cancelled. Each one's
    // clock ends as it's stopped, though its box is still held and its count hasn't ended:
    // read twice, 20 ms apart by the test's clock, the same; and its count's end, once the
    // box is let go, keeps it.
    let wait = std::time::Duration::from_secs(60);
    let block = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    let checked = |text: &str| Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    for stop in ["superseded", "cancelled"] {
        let engine = Recompute::new(FakeKernel::new(2).unwrap());
        let hold = engine
            .kernel()
            .hold_past_supersede(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
        let first = engine.start(checked(block));
        assert!(engine.kernel().wait_running(1, wait));
        spin(std::time::Duration::from_millis(5));
        let newer = (stop == "superseded").then(|| {
            engine.start(checked(
                &block.replace(r#""length": 40"#, r#""length": 41"#),
            ))
        });
        if newer.is_none() {
            engine.cancel();
        }
        let progress = first.progress();
        let at_stop = progress.now();
        assert!(!at_stop.ended, "{stop}: its box is still held");
        spin(std::time::Duration::from_millis(20));
        assert_eq!(progress.now().elapsed, at_stop.elapsed, "{stop}");
        hold.release();
        assert!(matches!(
            first.wait_timeout(wait),
            Ok(Ok(RunOutcome::Superseded))
        ));
        let ended = progress.now();
        assert!(ended.ended, "{stop}");
        assert_eq!(
            ended.elapsed, at_stop.elapsed,
            "{stop}: its end kept its stop's time"
        );
        if let Some(newer) = newer {
            assert!(matches!(
                newer.wait_timeout(wait),
                Ok(Ok(RunOutcome::Published(_)))
            ));
        }
    }
}
