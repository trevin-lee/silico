//! A pattern's step at the cap (docs/design/m6.0-patterns.md, section 2.4, *Time at the
//! cap*): 1,000 slots pocketed in a 40 by 25 grid, 999 copies of the slot's tool cut in one
//! boolean, the bar half the kernel's 60-second timeout. Three times:
//!
//! - `step`: the copies request alone, on one worker, its base the pocketed plate and its
//!   tool a box the slot's through-all prism is, the time the step holds a worker;
//! - `rebuild`: the pattern added to a rebuilt plate, on the same engine, so only its step
//!   runs, then its solid's measure (a summary, and every face, edge, and vertex described);
//! - `candidates`: a chamfer on a copy's rim the grid doesn't have, its `reference_broken`
//!   and candidates found among the 1,000 instances' edges (`Candidates::of_name`, O(B)).
//!
//! Each is checked (the plate's volume less 1,000 slots', by hand; the chamfer broken, with
//! candidates), and its time printed; the step's is held to the bar. Run with
//! `pixi run cargo bench -p silico-kernel --bench patterns`.

use std::error::Error;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::{CopiesOp, KernelRequest, KernelWorker, Placement, Primitive};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// Half the kernel's timeout: the bar a step at the cap is held to.
const BAR: Duration = Duration::from_secs(30);

/// The plate: 210 × 260 × 5 from the origin, a 2 × 6 slot at (5, 5) pocketed through it.
const PLATE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Plate", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 210, "y": 0, "construction": false}, {"id": "pa000003", "x": 210, "y": 260, "construction": false}, {"id": "pa000004", "x": 0, "y": 260, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []},
{"id": "p0000001", "type": "pad", "v": 1, "name": "Plate", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}},
{"id": "s0000002", "type": "sketch", "v": 1, "name": "Slot", "plane": "xy", "points": [{"id": "pb000001", "x": 5, "y": 5, "construction": false}, {"id": "pb000002", "x": 7, "y": 5, "construction": false}, {"id": "pb000003", "x": 7, "y": 11, "construction": false}, {"id": "pb000004", "x": 5, "y": 11, "construction": false}], "lines": [{"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}, {"id": "nb000002", "from": "pb000002", "to": "pb000003", "construction": false}, {"id": "nb000003", "from": "pb000003", "to": "pb000004", "construction": false}, {"id": "nb000004", "from": "pb000004", "to": "pb000001", "construction": false}], "circles": [], "arcs": [], "constraints": []},
{"id": "q0000001", "type": "pocket", "v": 1, "name": "Slot", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "through_all", "reversed": true}}"#;

/// 40 columns 5 apart along x, by 25 rows 10 apart along y: 1,000 instances.
const GRID: &str = r#"{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "Slots", "base": "q0000001", "copies": {"type": "features", "features": ["q0000001"]}, "directions": [{"along": "x", "reversed": false, "distribution": {"type": "spacing", "length": 5}, "count": 40}, {"along": "y", "reversed": false, "distribution": {"type": "spacing", "length": 10}, "count": 25}], "skip": []}"#;

/// A chamfer on copy (40, 1)'s rim, past direction 1's count of 40.
const CHAMFER: &str = r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Rim", "base": "t0000001", "edges": ["t0000001.linear_pattern.Edge(G p0000001.pad.Face(G s0000001.sketch.Face1), G t0000001.linear_pattern.Face(C40,1 q0000001.pocket.Face(G s0000002.sketch.Edge.nb000001)))"], "size": {"distance": 0.5}}"#;

fn document(features: &[&str]) -> Result<Checked, Box<dyn Error>> {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}]}}",
        features.join(",\n")
    );
    Ok(Checked::new(read(text.as_bytes())?).map_err(|e| format!("{:?}", e.errors()))?)
}

fn feature(text: &str) -> Result<FeatureId, Box<dyn Error>> {
    Ok(FeatureId::new(text).ok_or("not an ID")?)
}

fn rebuilt(
    engine: &Recompute<KernelPool>,
    features: &[&str],
) -> Result<(Arc<Model>, Duration), Box<dyn Error>> {
    let start = Instant::now();
    match engine
        .start(document(features)?)
        .wait_timeout(Duration::from_secs(600))
    {
        Ok(Ok(RunOutcome::Published(model))) => Ok((model, start.elapsed())),
        _ => Err("the rebuild didn't publish".into()),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = std::io::stdout().lock();
    let engine = Recompute::new(KernelPool::start_workers(Path::new(WORKER), 2)?)
        .with_sketches(FakeSketches::new());
    let (plate, took) = rebuilt(&engine, &[PLATE])?;
    writeln!(out, "plate: {took:?}")?;
    let Some(FeatureOutcome::Built { built: base, .. }) = plate.outcome(&feature("q0000001")?)
    else {
        return Err("the plate didn't build".into());
    };

    // The step alone: the copies request the pattern sends, its tool the slot's prism.
    let mut worker = KernelWorker::start_worker(Path::new(WORKER))?;
    let tool = worker.run(KernelRequest::make(Primitive::Box {
        dx: 2.0,
        dy: 6.0,
        dz: 7.0,
    })?)?;
    let tool = worker
        .run(KernelRequest::translate(&tool, [5.0, 5.0, -1.0])?)?
        .shape;
    let placements: Vec<Placement> = (0..40u32)
        .flat_map(|i| (0..25u32).map(move |j| (i, j)))
        .filter(|&(i, j)| (i, j) != (0, 0))
        .map(|(i, j)| Placement::Translation {
            offset: [5.0 * f64::from(i), 10.0 * f64::from(j), 0.0],
        })
        .collect();
    assert_eq!(placements.len(), 999);
    let start = Instant::now();
    let made = worker.run(KernelRequest::copies(
        CopiesOp::Cut,
        &base.shape,
        &tool,
        &placements,
    )?)?;
    let step = start.elapsed();
    writeln!(
        out,
        "step: {step:?} (999 copies, {} faces)",
        made.history.result.faces
    )?;

    // The pattern rebuilt on the plate: its step and its measure.
    let (model, took) = rebuilt(&engine, &[PLATE, GRID])?;
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&feature("t0000001")?) else {
        return Err(format!(
            "the grid didn't build: {:?}",
            model.outcome(&feature("t0000001")?)
        )
        .into());
    };
    let volume = built.summary().ok_or("unmeasured")?.volume;
    // 210 · 260 · 5 less 1,000 slots of 2 · 6 · 5.
    let want = 273_000.0 - 1000.0 * 60.0;
    if (volume - want).abs() > 1e-9 * want {
        return Err(format!("the grid's volume is {volume}, not {want}").into());
    }
    writeln!(
        out,
        "rebuild: {took:?} (the step, and its solid measured and described)"
    )?;

    // A broken name among the 1,000 instances' edges, with its candidates.
    let (model, took) = rebuilt(&engine, &[PLATE, GRID, CHAMFER])?;
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&feature("c0000001")?) else {
        return Err("the chamfer isn't broken".into());
    };
    if errors.first().is_none_or(|e| e.candidates().is_empty()) {
        return Err("the broken name has no candidates".into());
    }
    writeln!(out, "candidates: {took:?} ({:?})", errors[0].candidates())?;
    if step > BAR {
        return Err(format!("a step at the cap took {step:?}, past the bar of {BAR:?}").into());
    }
    Ok(())
}
