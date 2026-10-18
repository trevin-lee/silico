//! Planning cost: what a recompute costs the core, apart from the kernel, along a chain of
//! fillets (a box, then fillet after fillet, each of the one before), the timeline every
//! part in Silico is (each feature consumes its base). Each length is rebuilt three ways
//! on the fake kernel: from nothing; again, unchanged (no step runs, so it's planning and
//! scheduling alone, the floor under every edit); and with the last fillet's radius changed
//! (two kernel calls). Recorded, never budgeted: it fails on no time. Run with
//! `pixi run cargo bench -p silico-core --bench plan`.
//!
//! The rebuild from nothing grows faster than its length on the fake alone: a fake shape
//! lists every step that made it, so each call reads and writes the chain so far. The
//! unchanged and edited rebuilds read none, and grow as the chain does.

use std::error::Error;
use std::io::Write as _;
use std::time::{Duration, Instant};

use silico_core::command;
use silico_core::fake::FakeKernel;
use silico_core::recompute::Recompute;

const LENGTHS: [usize; 4] = [500, 1000, 2000, 4000];

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Feature `n`'s ID, in the ID alphabet.
fn id(n: usize) -> String {
    let alphabet = silico_core::document::ID_ALPHABET.as_bytes();
    let mut v = n;
    (0..8)
        .map(|_| {
            let c = char::from(alphabet[v % alphabet.len()]);
            v /= alphabet.len();
            c
        })
        .collect()
}

/// A box and `n` fillets, each of the one before, of one of the box's edges, the last of
/// radius `last`.
fn chain(n: usize, last: f64) -> String {
    let mut features = vec![format!(
        r#"{{"id": "{}", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}}"#,
        id(0)
    )];
    for i in 1..=n {
        let radius = if i == n { last } else { 1.0 };
        features.push(format!(
            r#"{{"id": "{}", "type": "fillet", "v": 1, "name": "Round", "base": "{}", "edges": ["{}.box.Edge{}"], "radius": {radius}}}"#,
            id(i),
            id(i - 1),
            id(0),
            i % 12 + 1
        ));
    }
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
        features.join(",\n")
    )
}

/// How long a rebuild of `text` on `engine` takes.
fn rebuilt(engine: &Recompute<FakeKernel>, text: &str) -> Result<Duration> {
    let checked = command::gate(text.as_bytes())
        .map_err(|problems| format!("{} problems", problems.total()))?;
    let started = Instant::now();
    let model = command::rebuild(engine, checked)
        .map_err(|e| e.to_string())?
        .ok_or("the rebuild was superseded")?;
    let took = started.elapsed();
    if model.error_count() > 0 {
        return Err(format!("{} errors", model.error_count()).into());
    }
    Ok(took)
}

fn main() -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "{:>6} {:>12} {:>12} {:>12} {:>6}",
        "length", "rebuild", "unchanged", "edit last", "calls"
    )?;
    for n in LENGTHS {
        let engine = Recompute::new(FakeKernel::new(4)?);
        let first = rebuilt(&engine, &chain(n, 1.0))?;
        let unchanged = rebuilt(&engine, &chain(n, 1.0))?;
        let before = engine.kernel().calls().len();
        let edited = rebuilt(&engine, &chain(n, 2.0))?;
        let calls = engine.kernel().calls().len() - before;
        writeln!(
            out,
            "{n:>6} {:>12.1?} {:>12.1?} {:>12.1?} {calls:>6}",
            first, unchanged, edited
        )?;
    }
    Ok(())
}
