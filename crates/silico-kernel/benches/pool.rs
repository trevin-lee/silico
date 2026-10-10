//! The worker pool's throughput (docs/roadmap/m1-kernel.md, M1.6): 10,000 small requests,
//! empty echoes and then 10 × 20 × 30 boxes, submitted at once to a pool as large as the
//! machine has cores, against the same requests one at a time on one worker. Recorded as the
//! baseline, not budgeted. Run with `pixi run cargo bench -p silico-kernel --bench pool`.

use std::error::Error;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use silico_core::kernel::{Blob, KernelRequest, KernelWorker, Primitive};
use silico_core::pool::{KernelPool, Pending};
use silico_protocol::Generation;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const REQUESTS: u32 = 10_000;

fn echo() -> KernelRequest<Blob> {
    KernelRequest::echo(Blob::inline(Vec::new()))
}

fn a_box() -> Result<KernelRequest<silico_core::kernel::ShapeData>, silico_core::Error> {
    KernelRequest::make(Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
}

/// Waits for every request, failing if any failed or was superseded.
fn wait_all<T>(pending: Vec<Pending<T>>) -> Result<(), Box<dyn Error>> {
    for p in pending {
        p.wait().done().ok_or("superseded")??;
    }
    Ok(())
}

fn per_second(elapsed: Duration) -> f64 {
    f64::from(REQUESTS) / elapsed.as_secs_f64()
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = std::io::stdout().lock();
    let size = std::thread::available_parallelism().map_or(1, usize::from);
    let pool = KernelPool::start_workers(Path::new(WORKER), size)?;
    let mut one = KernelWorker::start_worker(Path::new(WORKER))?;

    // Warm-up, so every worker has loaded OCCT.
    let warm = (0..size * 10)
        .map(|_| a_box().map(|r| pool.submit(Generation(1), r)))
        .collect::<Result<Vec<_>, _>>()?;
    wait_all(warm)?;
    one.run(a_box()?)?;

    let start = Instant::now();
    wait_all(
        (0..REQUESTS)
            .map(|_| pool.submit(Generation(1), echo()))
            .collect(),
    )?;
    let pooled = start.elapsed();
    let start = Instant::now();
    for _ in 0..REQUESTS {
        one.run(echo())?;
    }
    let single = start.elapsed();
    writeln!(
        out,
        "echo: pool of {size} {:.0}/s ({pooled:?}), one worker {:.0}/s ({single:?})",
        per_second(pooled),
        per_second(single)
    )?;

    let start = Instant::now();
    let boxes = (0..REQUESTS)
        .map(|_| a_box().map(|r| pool.submit(Generation(1), r)))
        .collect::<Result<Vec<_>, _>>()?;
    wait_all(boxes)?;
    let pooled = start.elapsed();
    let start = Instant::now();
    for _ in 0..REQUESTS {
        one.run(a_box()?)?;
    }
    let single = start.elapsed();
    writeln!(
        out,
        "box: pool of {size} {:.0}/s ({pooled:?}), one worker {:.0}/s ({single:?})",
        per_second(pooled),
        per_second(single)
    )?;
    Ok(())
}
