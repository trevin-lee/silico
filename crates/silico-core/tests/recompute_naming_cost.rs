//! MA1 (the milestone's architecture review): a fillet's names are read from the maps
//! already named, so naming a chain of fillets costs a map or two a fillet, not every map
//! before it (planning was quadratic in the chain). Counted from each `resolve` span's
//! `sources`, the maps a fillet's names were read from, so the bound holds to no clock.
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

/// Each `resolve` span's `sources`.
#[derive(Clone, Default)]
struct Sources(Arc<Mutex<Vec<u64>>>);

impl Sources {
    fn take(&self) -> Vec<u64> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct Count(u64);

impl Visit for Count {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "sources" {
            self.0 = value;
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Sources {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        if attrs.metadata().name() == "resolve" {
            let mut count = Count(u64::MAX);
            attrs.record(&mut count);
            self.0.lock().unwrap().push(count.0);
        }
    }
}

/// A box (`b0000000`) and `n` fillets, each of the one before, of one of the box's edges,
/// the last of radius `last`.
fn chain(n: usize, last: f64) -> Checked {
    let id = |i: usize| format!("f{i:07}");
    let mut features = vec![
        r#"{"id": "b0000000", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#.to_owned(),
    ];
    for i in 1..=n {
        let base = if i == 1 {
            "b0000000".to_owned()
        } else {
            id(i - 1)
        };
        let radius = if i == n { last } else { 1.0 };
        features.push(format!(
            r#"{{"id": "{}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": ["b0000000.box.Edge{}"], "radius": {radius}}}"#,
            id(i),
            i % 12 + 1
        ));
    }
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
        features.join(",\n")
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn rebuilt(engine: &Recompute<FakeKernel>, document: Checked) {
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => {
            assert!(
                model.errors().next().is_none(),
                "{:?}",
                model.errors().collect::<Vec<_>>()
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_chain_of_fillets_reads_each_fillets_names_from_a_map_or_two() {
    let seen = Sources::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(seen.clone()))
        .unwrap();
    let n = 300;
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    // From nothing: each fillet's base is named from its solid, its base's base read as
    // named (the first's base, the box, a primitive, from its solid alone).
    rebuilt(&engine, chain(n, 1.0));
    let sources = seen.take();
    assert_eq!(sources.len(), n);
    assert!(sources.iter().all(|&s| (1..=2).contains(&s)), "{sources:?}");
    // The last fillet's radius changed: it alone is resolved, its base's map named already.
    rebuilt(&engine, chain(n, 2.0));
    assert_eq!(seen.take(), [1]);
}
