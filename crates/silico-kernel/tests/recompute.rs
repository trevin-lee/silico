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
    let pool =
        KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
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

#[test]
fn the_example_on_occt_is_the_block_with_the_pin_cut_through_it() {
    // By hand: a 40 x 20 x 10 block less a cylinder of radius 2.5 through its full height
    // (the pin runs from z = -1 to 11): volume 8000 - 62.5 pi; area the block's 2800, less
    // the two discs (12.5 pi), plus the hole's wall (50 pi); seven faces, the hole's one;
    // fifteen edges (the block's, the hole's two circles and its seam); ten vertices.
    let pool =
        KernelPool::start_on_demand(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
            .unwrap();
    let engine = Recompute::new(pool);
    let model = match engine.start(example()).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    };
    let hole = FeatureId::new("m3tq8v0z").unwrap();
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&hole) else {
        panic!("{model:?}")
    };
    let summary = engine
        .kernel()
        .submit(
            engine.kernel().generation(),
            silico_core::kernel::KernelRequest::summarize(&built.shape),
        )
        .wait()
        .done()
        .unwrap()
        .unwrap();
    let pi = std::f64::consts::PI;
    let close = |got: f64, want: f64| (got - want).abs() <= want * 1e-7;
    assert!(close(summary.volume, 8000.0 - 62.5 * pi), "{summary:?}");
    assert!(close(summary.area, 2800.0 + 37.5 * pi), "{summary:?}");
    assert_eq!(
        (summary.faces, summary.edges, summary.vertices),
        (7, 15, 10),
        "{summary:?}"
    );
    // The hole is through the block's centre, so the centre of mass is the block's.
    for (got, want) in summary.center.into_iter().zip([20.0, 10.0, 5.0]) {
        assert!((got - want).abs() <= 1e-9, "{summary:?}");
    }
    assert!(summary.valid);
}
