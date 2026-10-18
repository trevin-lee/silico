//! M2.6: fingerprints on OCCT. The example's features measured by the kernel against their
//! formulas (a box exactly; a cylinder, and a box with a hole through it, to within the
//! kernel's integration), their element counts worked by hand; and a saved document rebuilt
//! by other sessions, with a cache or without, to the same bytes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/cached.rs"]
mod cached;
#[path = "../../silico-core/tests/common/example.rs"]
mod example;

use std::f64::consts::PI;
use std::path::Path;

use cached::{Counted, Scratch};
use silico_core::document::{Checked, Document, FeatureId, Fingerprint, Length, read, write};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn pool() -> KernelPool {
    KernelPool::start_workers(Path::new(WORKER), 2).unwrap()
}

fn published(
    engine: &Recompute<impl silico_core::kernel::Kernel + 'static>,
    document: &Document,
) -> std::sync::Arc<Model> {
    match engine
        .start(Checked::new(document.clone()).unwrap())
        .wait()
        .unwrap()
    {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

fn fingerprint(model: &Model, id: &str) -> Fingerprint {
    model.fingerprint(&FeatureId::new(id).unwrap()).unwrap()
}

/// Whether a measure, in millionths, is within `slack` millionths of `want`.
fn near(millionths: i128, want: f64, slack: f64) -> bool {
    (millionths as f64 - want * 1e6).abs() <= slack
}

fn bbox(corners: [f64; 6]) -> Option<[Length; 6]> {
    Some(corners.map(|mm| Length::from_nm((mm * 1e6) as i64)))
}

#[test]
fn the_examples_features_measure_as_their_formulas() {
    let document = read(example::EXAMPLE.as_bytes()).unwrap();
    let model = published(&Recompute::new(pool()), &document);
    // The block, 40 by 20 by 10: exactly.
    let block = fingerprint(&model, "k7f2qx3m");
    assert_eq!(block.volume.to_string(), "8000");
    assert_eq!(block.area.to_string(), "2800");
    assert_eq!(block.bbox, bbox([0.0, 0.0, 0.0, 40.0, 20.0, 10.0]));
    assert_eq!((block.faces, block.edges, block.vertices), (6, 12, 8));
    // The pin, r 2.5 and h 12 from (20, 10, -1): its side and two caps, two circles and a
    // seam, the seam's two ends.
    let pin = fingerprint(&model, "a91cw0de");
    let (r, h) = (2.5, 12.0);
    assert!(
        near(pin.volume.millionths(), PI * r * r * h, 10.0),
        "{pin:?}"
    );
    assert!(
        near(
            pin.area.millionths(),
            2.0 * PI * r * h + 2.0 * PI * r * r,
            10.0
        ),
        "{pin:?}"
    );
    assert_eq!(pin.bbox, bbox([17.5, 7.5, -1.0, 22.5, 12.5, 11.0]));
    assert_eq!((pin.faces, pin.edges, pin.vertices), (3, 3, 2));
    // The block with a hole of r 2.5 through its 10: less the hole's volume, less its two
    // ends' area and plus its wall's; the box's faces and the wall, the box's edges, the
    // hole's two circles and its seam, the box's corners and the seam's ends.
    let hole = fingerprint(&model, "m3tq8v0z");
    assert!(
        near(hole.volume.millionths(), 8000.0 - PI * r * r * 10.0, 20.0),
        "{hole:?}"
    );
    assert!(
        near(
            hole.area.millionths(),
            2800.0 - 2.0 * PI * r * r + 2.0 * PI * r * 10.0,
            20.0
        ),
        "{hole:?}"
    );
    assert_eq!(hole.bbox, bbox([0.0, 0.0, 0.0, 40.0, 20.0, 10.0]));
    assert_eq!((hole.faces, hole.edges, hole.vertices), (7, 15, 10));
}

#[test]
fn a_saved_document_rebuilds_to_the_same_bytes_with_a_cache_or_without() {
    let document = read(example::EXAMPLE.as_bytes()).unwrap();
    let first = published(&Recompute::new(pool()), &document);
    let saved = write(&first.saved()).unwrap();
    let saved_document = read(saved.as_bytes()).unwrap();
    // Another session, on another pool.
    let again = published(&Recompute::new(pool()), &saved_document);
    assert!(again.drift().is_empty());
    assert_eq!(write(&again.saved()).unwrap(), saved);
    // Through a cache: built into it, then taken from it.
    let dir = Scratch::new("fingerprint");
    for (pass, asks) in [(1, 7), (2, 0)] {
        let engine = Recompute::with_cache(Counted::new(WORKER, 2), dir.cache());
        let cached = published(&engine, &saved_document);
        // Four steps made, three features' solids measured; then nothing.
        assert_eq!(engine.kernel().asked(), asks, "pass {pass}");
        assert!(cached.drift().is_empty());
        assert_eq!(write(&cached.saved()).unwrap(), saved);
    }
}
