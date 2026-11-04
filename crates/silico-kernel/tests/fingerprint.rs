//! M2.6: fingerprints on OCCT. The example's features measured by the kernel against their
//! formulas (a box exactly; a cylinder, and a box with a hole through it, to within the
//! kernel's integration), their element counts worked by hand; a saved document rebuilt
//! by other sessions, with a cache or without, to the same bytes; and a fillet on one edge
//! and on its mirror image, alike in every measure but their centres, worked by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::cached;
use super::example;

use std::f64::consts::PI;
use std::path::Path;

use cached::{Counted, Scratch};
use silico_core::document::{Checked, Document, FeatureId, Fingerprint, Length, read, write};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

fn pool() -> KernelPool {
    KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap()
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

fn centre(point: [f64; 3]) -> Option<[Length; 3]> {
    Some(point.map(|mm| Length::from_nm((mm * 1e6) as i64)))
}

/// A block 20 × 20 × 10 centred on the z axis, from (-10, -10, 0), filleted r 2 on the
/// vertical edge `edge`: OCCT's box numbers its vertical edges Edge1 at (x0, y0), Edge3 at
/// (x0, y1), Edge5 at (x1, y0), and Edge7 at (x1, y1).
fn filleted(edge: &str) -> Document {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Block", "position": [-10, -10, 0], "length": 20, "width": 20, "height": 10}}, {{"id": "ffffffff", "type": "fillet", "v": 1, "name": "Round", "base": "aaaaaaaa", "edges": ["aaaaaaaa.box.{edge}"], "radius": 2}}]}}"#
    );
    read(text.as_bytes()).unwrap()
}

#[test]
fn a_fillet_on_an_edge_and_on_its_mirror_image_differ_only_in_their_centres() {
    // The gap the centre closes: a fillet at (+10, +10) and one at (-10, +10), mirror images
    // across x = 0, have the same volume, area, box, and counts, so a kernel change that moved
    // a fillet onto its twin passed every other measure.
    //
    // By hand: the fillet takes from the corner a sliver of ΔV = (1 - π/4) r² h, whose
    // centroid is k r in from the corner on each axis, k = (5/6 - π/4) / (1 - π/4) (the r × r
    // square's moment, r/2 out, less the quarter disc's, r - 4r/3π out). The block's centre,
    // (0, 0, 5), moves away from the corner by c = ΔV (10 - k r) / (V - ΔV), V = 4000: so
    // (-c, -c, 5) for the fillet at (+10, +10) and (+c, -c, 5) for the one at (-10, +10).
    let (r, h, volume) = (2.0, 10.0, 4000.0);
    let sliver = (1.0 - PI / 4.0) * r * r * h;
    let k = (5.0 / 6.0 - PI / 4.0) / (1.0 - PI / 4.0);
    let c = sliver * (10.0 - k * r) / (volume - sliver);
    // k = 0.22336..., c = 0.0205456 mm: 20546 nm, rounded.
    assert_eq!((k * 1e5).round(), 22_337.0);
    assert_eq!((c * 1e6).round(), 20_546.0);
    let pool = pool();
    let engine = Recompute::new(pool);
    let plus = fingerprint(&published(&engine, &filleted("Edge7")), "ffffffff");
    let minus = fingerprint(&published(&engine, &filleted("Edge3")), "ffffffff");
    // Each to within a nanometre of the hand value (its rounding and the integration's).
    let near_nm = |got: Option<[Length; 3]>, want: [f64; 3]| {
        let got = got.unwrap().map(Length::nm);
        (0..3).all(|i| (got[i] as f64 - want[i] * 1e6).abs() <= 1.0)
    };
    assert!(near_nm(plus.center, [-c, -c, 5.0]), "{plus:?}");
    assert!(near_nm(minus.center, [c, -c, 5.0]), "{minus:?}");
    // Mirror images to the nanometre: x negated, y and z alike.
    let (p, m) = (plus.center.unwrap(), minus.center.unwrap());
    assert_eq!(
        (p[0].nm(), p[1], p[2]),
        (-m[0].nm(), m[1], m[2]),
        "{plus:?} {minus:?}"
    );
    // Everything else alike: the volume (V - ΔV) and area by hand, to within the
    // integration; the box the block's; the block's 6 faces, 12 edges, and 8 vertices, and
    // the fillet's face, its two arcs' edges and their ends, less the edge it replaced.
    assert!(
        near(plus.volume.millionths(), volume - sliver, 10.0),
        "{plus:?}"
    );
    let area =
        2.0 * 400.0 + 4.0 * 200.0 - 2.0 * (1.0 - PI / 4.0) * r * r - 2.0 * r * h + PI / 2.0 * r * h;
    assert!(near(plus.area.millionths(), area, 10.0), "{plus:?}");
    assert_eq!(plus.bbox, bbox([-10.0, -10.0, 0.0, 10.0, 10.0, 10.0]));
    assert_eq!((plus.faces, plus.edges, plus.vertices), (7, 15, 10));
    let fields: Vec<_> = plus
        .differences(&minus)
        .into_iter()
        .map(|d| d.field)
        .collect();
    assert_eq!(fields, ["center"]);
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
    assert_eq!(block.center, centre([20.0, 10.0, 5.0]));
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
    // On its axis, halfway up from -1 to 11.
    assert_eq!(pin.center, centre([20.0, 10.0, 5.0]));
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
    // The hole is on the block's axis, (20, 10), through its whole height: the centre stays.
    assert_eq!(hole.center, centre([20.0, 10.0, 5.0]));
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
    for (pass, asks) in [(1, 10), (2, 0)] {
        let engine = Recompute::with_cache(Counted::new(WORKER, 2), dir.cache());
        let cached = published(&engine, &saved_document);
        // Four steps made, three features' solids measured (each a summary and one request
        // describing it: 4 + 3 × 2); then nothing.
        assert_eq!(engine.kernel().asked(), asks, "pass {pass}");
        assert!(cached.drift().is_empty());
        assert_eq!(write(&cached.saved()).unwrap(), saved);
    }
}
