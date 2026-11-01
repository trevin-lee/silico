//! M3.4: a solved sketch is its own solution. A sketch is solved on every load from the
//! coordinates its file stores, and the solve's points and radii are what `silico fmt`
//! writes back; so solving a sketch, writing its solution as the file does, and solving
//! again must give the same values bit for bit, and the same file, or an unchanged sketch
//! would show drift on every load. Checked over the oracle's sketches
//! (tests/oracle/sketches.silico), each as stored and from twenty starts drawn off it;
//! silico-sketch's tests/stress.rs checks the same of the worker alone, over its corpus.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::systems;

use std::path::Path;

use silico_core::document::{
    Checked, Coordinate, Document, FeatureId, Operation, Sketch, read, write,
};
use silico_core::sketch::{SketchSolution, SketchWorker, solve};
use systems::Rng;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const ORACLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/oracle/sketches.silico"
);
const STARTS: usize = 20;

fn sketch_mut<'a>(document: &'a mut Document, feature: &FeatureId) -> &'a mut Sketch {
    let found = document
        .features
        .iter_mut()
        .find(|f| &f.id == feature)
        .unwrap();
    match &mut found.operation {
        Operation::Sketch(s) => s,
        other => panic!("{other:?}"),
    }
}

/// `document` with `feature`'s points and circles' radii where `solution` has them.
fn written_back(document: &Document, feature: &FeatureId, solution: &SketchSolution) -> Document {
    let mut out = document.clone();
    let sketch = sketch_mut(&mut out, feature);
    for p in &mut sketch.points {
        [p.x, p.y] = solution.points[&p.id];
    }
    for c in &mut sketch.circles {
        c.radius = solution.radii[&c.id];
    }
    out
}

/// The solution's values, as bits, in a fixed order: each point's x and y, then each
/// radius, by ID.
fn bits(solution: &SketchSolution) -> Vec<u64> {
    let points = solution
        .points
        .values()
        .flat_map(|[x, y]| [x.mm().to_bits(), y.mm().to_bits()]);
    let radii = solution.radii.values().map(|r| r.mm().to_bits());
    points.chain(radii).collect()
}

/// `document`, drawn off: each of `feature`'s points moved up to 2% of the sketch's size
/// in each direction, from `rng`.
fn drawn_off(document: &Document, feature: &FeatureId, rng: &mut Rng) -> Document {
    let mut out = document.clone();
    let sketch = sketch_mut(&mut out, feature);
    let (lo, hi) = sketch
        .points
        .iter()
        .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| {
            (
                [lo[0].min(p.x.mm()), lo[1].min(p.y.mm())],
                [hi[0].max(p.x.mm()), hi[1].max(p.y.mm())],
            )
        });
    let off = 0.02 * (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0);
    for p in &mut sketch.points {
        p.x = Coordinate::new(p.x.mm() + rng.range(-off, off)).unwrap();
        p.y = Coordinate::new(p.y.mm() + rng.range(-off, off)).unwrap();
    }
    out
}

/// Solves `feature` of `document`, writes its solution back through the file's text, and
/// solves that: the same values, bit for bit, and the same text. Whether the first solve
/// solved (a case drawn too far off may not).
fn holds(document: &Document, feature: &FeatureId, w: &mut SketchWorker, case: &str) -> bool {
    let Ok(first) = solve(&Checked::new(document.clone()).unwrap(), feature, w) else {
        return false;
    };
    let text = write(&written_back(document, feature, &first)).unwrap();
    let reread = read(text.as_bytes()).unwrap();
    let second = solve(&Checked::new(reread.clone()).unwrap(), feature, w)
        .unwrap_or_else(|e| panic!("{case}: solved once, then not from its solution: {e}"));
    assert_eq!(
        bits(&second),
        bits(&first),
        "{case}: solving from its solution moved it"
    );
    assert_eq!(
        write(&written_back(&reread, feature, &second)).unwrap(),
        text,
        "{case}: the file changed"
    );
    true
}

#[test]
fn every_oracle_sketch_solved_is_its_own_solution_as_stored_and_from_twenty_starts() {
    let oracle = read(&std::fs::read(ORACLE).unwrap()).unwrap();
    let features: Vec<(FeatureId, String)> = oracle
        .features
        .iter()
        .map(|f| (f.id.clone(), f.name.as_str().to_owned()))
        .collect();
    assert_eq!(features.len(), 18, "the oracle's sketch cases");
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let mut rng = Rng(0x0f19_ed90);
    let (mut solved, mut tried) = (0, 0);
    for (feature, name) in &features {
        // Each feature on its own, so one that doesn't solve can't stop the others.
        let mut alone = oracle.clone();
        alone.features.retain(|f| &f.id == feature);
        tried += 1;
        solved += usize::from(holds(&alone, feature, &mut w, name));
        for start in 0..STARTS {
            let off = drawn_off(&alone, feature, &mut rng);
            tried += 1;
            solved += usize::from(holds(
                &off,
                feature,
                &mut w,
                &format!("{name}, start {start}"),
            ));
        }
    }
    // The oracle's cases include ones meant not to solve (conflicting, over-dimensioned),
    // and a start drawn off may fail; but most solve, so the check can't pass empty.
    assert!(solved >= 200, "only {solved} of {tried} sketches solved");
}
