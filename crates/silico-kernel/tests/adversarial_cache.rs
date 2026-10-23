//! break-it: the M2.5 rebuild cache on OCCT. A hit must be byte-identical to what the
//! kernel makes for the same step: for every primitive and boolean, and when the kernel
//! builds on hits (an edit whose unchanged inputs come from the cache). The independent
//! source is a session with no cache at all, on its own pool.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::cached;
use super::example;

use std::sync::Arc;

use cached::{Counted, Scratch};
use silico_core::document::{Checked, read};
use silico_core::kernel::Kernel;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn counted() -> Counted {
    Counted::new(WORKER, 2)
}

fn published<K: Kernel + 'static>(engine: &Recompute<K>, text: &str) -> Arc<Model> {
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

/// Every primitive, positioned and not, and every boolean, with a chain of booleans on
/// booleans.
const EVERY_KIND: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "00000000", "type": "sphere", "v": 1, "name": "S", "position": [1, 2, 3], "radius": 4},
    {"id": "11111111", "type": "cone", "v": 1, "name": "C", "position": [0, 0, 0], "radius1": 5, "radius2": 0, "height": 7.25},
    {"id": "22222222", "type": "torus", "v": 1, "name": "T", "position": [-0.5, 0, 0], "radius1": 10, "radius2": 2},
    {"id": "33333333", "type": "fuse", "v": 1, "name": "F", "base": "00000000", "tool": "11111111"},
    {"id": "44444444", "type": "box", "v": 1, "name": "B", "position": [-20, -20, -1], "length": 40, "width": 40, "height": 3},
    {"id": "55555555", "type": "common", "v": 1, "name": "M", "base": "44444444", "tool": "22222222"},
    {"id": "66666666", "type": "cylinder", "v": 1, "name": "Y", "position": [0, 0, -5], "radius": 1.5, "height": 20},
    {"id": "77777777", "type": "cut", "v": 1, "name": "H", "base": "33333333", "tool": "66666666"}
  ]
}
"#;

fn shapes(model: &Model) -> Vec<(String, Vec<u8>, Option<bool>)> {
    model
        .features()
        .iter()
        .map(|(id, outcome)| match outcome {
            FeatureOutcome::Built { built, .. } => (
                id.as_str().to_owned(),
                built.shape.brep().as_bytes().to_vec(),
                built.shape.valid(),
            ),
            other => panic!("{id:?}: {other:?}"),
        })
        .collect()
}

#[test]
fn every_kind_of_step_is_rebuilt_from_the_cache_byte_for_byte() {
    let dir = Scratch::new("every-kind");
    let fresh = published(&Recompute::new(counted()), EVERY_KIND);
    let first = Recompute::with_cache(counted(), dir.cache());
    let built = published(&first, EVERY_KIND);
    drop(first);
    let second = Recompute::with_cache(counted(), dir.cache());
    let again = published(&second, EVERY_KIND);
    assert_eq!(second.kernel().asked(), 0);
    assert_eq!(shapes(&built), shapes(&fresh));
    assert_eq!(shapes(&again), shapes(&fresh));
    assert_eq!(again.features(), fresh.features());
    assert_eq!(again.bodies(), fresh.bodies());
}

#[test]
fn an_edit_built_on_hits_is_byte_identical_to_one_built_fresh() {
    let dir = Scratch::new("edit-on-hits");
    let edited = example::EXAMPLE.replace(r#""radius": 2.5"#, r#""radius": 3.25"#);
    let fresh = published(&Recompute::new(counted()), &edited);
    published(
        &Recompute::with_cache(counted(), dir.cache()),
        example::EXAMPLE,
    );
    // The block comes from the cache, as bytes read from disk; the cut is OCCT's, on them.
    let second = Recompute::with_cache(counted(), dir.cache());
    let model = published(&second, &edited);
    // Three steps made (the pin's make and move, the cut), and two solids measured (the
    // pin's, its move's, and the cut's).
    assert_eq!(second.kernel().asked(), 5);
    assert_eq!(shapes(&model), shapes(&fresh));
    assert_eq!(model.features(), fresh.features());
    // And again from the cache, now entirely.
    let third = Recompute::with_cache(counted(), dir.cache());
    let model = published(&third, &edited);
    assert_eq!(third.kernel().asked(), 0);
    assert_eq!(model.features(), fresh.features());
}
