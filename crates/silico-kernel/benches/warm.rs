//! A first rebuild's time, with and without a warm start (docs/design/m5.0-app.md, section
//! 10, decision 10): the bracket's L profile padded, rebuilt by a fresh engine on a fresh
//! kernel pool and sketch solvers, no cache, so it pays for the workers it starts. Three
//! ways:
//!
//! - `cold`: the workers start on demand, as the rebuild needs them;
//! - `warm`: `KernelPool::warm(1)` and `SketchSolvers::warm(1)` first (as `Sessions::warm`
//!   makes them), which return once both workers' processes have started, then the rebuild (the app's
//!   case: they start while the person opens a file);
//! - `warm-now`: the rebuild at once after the warm start, while the workers start, which
//!   waits for them rather than starting more.
//!
//! Recorded, never budgeted: it fails on no time. Run with
//! `SILICO_SKETCH_WORKER=target/release/silico-sketch-worker pixi run cargo bench -p silico-kernel --bench warm`
//! after `pixi run cargo build --release -p silico-sketch --bin silico-sketch-worker`
//! (bench.yml's `warm`).

use std::error::Error;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::document::{Checked, read};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Recompute, RunOutcome, SketchSolvers};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const RUNS: usize = 15;

/// The bracket's L profile on the XZ plane, padded 40 along +Y (M4.1's example).
const BRACKET: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Legs", "sketch": "s0000001", "extent": {"type": "one_side", "length": 40, "reversed": true}}]}"#;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Cold,
    Warm,
    WarmNow,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Cold => "cold",
            Mode::Warm => "warm",
            Mode::WarmNow => "warm-now",
        }
    }
}

/// One first rebuild in `mode`: its time, from the rebuild's start to its model.
fn first_rebuild(document: &Arc<Checked>, mode: Mode) -> Result<Duration, Box<dyn Error>> {
    // As `Sessions` holds them: a pool, its rebuilds each a client of it, and solvers every
    // rebuild shares; warmed as `Sessions::warm` warms them.
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 1)?;
    let solvers = SketchSolvers::workers();
    let engine = Recompute::new(pool.client()).with_solvers(solvers.clone());
    // The two at once, as `Sessions::warm` makes them: the kernel's on a thread of its own,
    // the sketch solvers' here.
    let warm = || {
        std::thread::scope(|scope| {
            scope.spawn(|| pool.warm(1));
            solvers.warm(1);
        });
    };
    let rebuild = || -> Result<Duration, Box<dyn Error>> {
        let started = Instant::now();
        match engine.start(Arc::clone(document)).wait()? {
            RunOutcome::Published(model) => {
                if model.error_count() > 0 {
                    return Err(format!("the bracket failed: {:?}", model.errors().next()).into());
                }
            }
            RunOutcome::Superseded => return Err("superseded".into()),
        }
        Ok(started.elapsed())
    };
    match mode {
        Mode::Cold => rebuild(),
        // The warm start returns once both workers' processes have started.
        Mode::Warm => {
            warm();
            rebuild()
        }
        Mode::WarmNow => std::thread::scope(|scope| {
            scope.spawn(warm);
            rebuild()
        }),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = std::io::stdout().lock();
    let document = Arc::new(Checked::new(read(BRACKET.as_bytes())?).map_err(|e| format!("{e:?}"))?);
    writeln!(
        out,
        "first rebuild of the padded bracket, {RUNS} runs a mode, interleaved"
    )?;
    writeln!(
        out,
        "{:<10} {:>10} {:>10} {:>10}",
        "mode", "min ms", "p50 ms", "max ms"
    )?;
    let modes = [Mode::Cold, Mode::Warm, Mode::WarmNow];
    let mut times: Vec<Vec<Duration>> = vec![Vec::new(); modes.len()];
    for _ in 0..RUNS {
        for (i, &mode) in modes.iter().enumerate() {
            times[i].push(first_rebuild(&document, mode)?);
        }
    }
    for (mode, mut t) in modes.into_iter().zip(times) {
        t.sort_unstable();
        let (Some(min), Some(max)) = (t.first(), t.last()) else {
            continue;
        };
        writeln!(
            out,
            "{:<10} {:>10.1} {:>10.1} {:>10.1}",
            mode.name(),
            ms(*min),
            ms(t.get(t.len() / 2).copied().unwrap_or_default()),
            ms(*max)
        )?;
    }
    Ok(())
}
