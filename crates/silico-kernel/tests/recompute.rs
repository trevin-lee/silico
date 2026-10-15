//! M2.4: recompute on the real worker pool (OCCT). The example document builds, and builds
//! to the same bytes again on another pool: recompute adds nothing that varies between runs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/example.rs"]
mod example;

use std::path::Path;
use std::sync::Arc;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

fn example() -> Checked {
    Checked::new(read(example::EXAMPLE.as_bytes()).unwrap()).unwrap()
}

fn built() -> Arc<Model> {
    let pool = KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
        .unwrap();
    match Recompute::new(pool).start(example()).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

#[test]
fn the_example_builds_on_occt_to_the_same_bytes_every_time() {
    let first = built();
    let hole = FeatureId::new("m3tq8v0z").unwrap();
    let Some(FeatureOutcome::Built { built: shape, .. }) = first.outcome(&hole) else {
        panic!("{first:?}")
    };
    assert_eq!(shape.shape.valid(), Some(true));
    assert_eq!(first.bodies(), [hole]);
    assert_eq!(first.errors().count(), 0);
    let second = built();
    assert_eq!(second.features(), first.features());
}
