//! The IPC budget (docs/roadmap/m0-foundations.md, M0.7): a small request's round trip
//! through a worker takes under 50 µs, and a 10 MB blob moves at over 1 GB/s. Run with
//! `pixi run cargo bench -p silico-kernel --bench ipc`; it exits with an error if either
//! budget is missed. Missing one is the trigger for a shared-memory `Blob`.

use std::error::Error;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use silico_core::worker::Worker;
use silico_protocol::{Blob, Op, Reply};

const ROUND_TRIP_BUDGET: Duration = Duration::from_micros(50);
const THROUGHPUT_BUDGET: f64 = 1e9; // bytes per second

fn echo(worker: &mut Worker, bytes: Vec<u8>) -> Result<usize, Box<dyn Error>> {
    match worker.call(
        1,
        Op::Echo {
            data: Blob::inline(bytes),
        },
        Duration::from_secs(30),
    )? {
        Reply::Echo { data } => Ok(data.as_bytes().len()),
        other => Err(format!("Echo replied {other:?}").into()),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = std::io::stdout().lock();
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")))?;

    // Small round trips: the median of 20,000, after a warm-up.
    for _ in 0..1_000 {
        echo(&mut worker, Vec::new())?;
    }
    let mut times = Vec::with_capacity(20_000);
    for _ in 0..20_000 {
        let start = Instant::now();
        echo(&mut worker, Vec::new())?;
        times.push(start.elapsed());
    }
    times.sort();
    let median = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    writeln!(
        out,
        "small round trip: median {median:?}, p99 {p99:?} (budget {ROUND_TRIP_BUDGET:?})"
    )?;

    // The real shape path: a box built (B-rep out through the bridge) and measured (B-rep
    // back in). Recorded, not budgeted: it's dominated by OCCT, not IPC.
    let mut shape_times = Vec::with_capacity(500);
    for _ in 0..500 {
        let start = Instant::now();
        let brep = match worker.call(
            1,
            Op::MakeBox {
                dx: 10.0,
                dy: 20.0,
                dz: 30.0,
            },
            Duration::from_secs(30),
        )? {
            Reply::Shape { brep } => brep,
            other => return Err(format!("MakeBox replied {other:?}").into()),
        };
        worker.call(
            1,
            Op::MassProperties { shape: brep },
            Duration::from_secs(30),
        )?;
        shape_times.push(start.elapsed());
    }
    shape_times.sort();
    writeln!(
        out,
        "box built and measured through the worker: median {:?}",
        shape_times[shape_times.len() / 2]
    )?;

    // Bulk: a 10 MB blob there and back, 20 times.
    let size = 10 * 1024 * 1024;
    echo(&mut worker, vec![7; size])?;
    let rounds = 20;
    let start = Instant::now();
    for _ in 0..rounds {
        if echo(&mut worker, vec![7; size])? != size {
            return Err("the echo came back the wrong size".into());
        }
    }
    let elapsed = start.elapsed();
    let throughput = (2 * size * rounds) as f64 / elapsed.as_secs_f64();
    writeln!(
        out,
        "10 MB echo: {:.2} GB/s, counting both directions (budget {:.1} GB/s)",
        throughput / 1e9,
        THROUGHPUT_BUDGET / 1e9
    )?;

    if median > ROUND_TRIP_BUDGET || throughput < THROUGHPUT_BUDGET {
        return Err("over the IPC budget".into());
    }
    Ok(())
}
