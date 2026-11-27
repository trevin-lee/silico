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
//!
//! Then a wide part: boxes in a row, each with a sketch on its top padded onto it, rebuilt
//! from nothing on fake kernels of one thread and of four, every face described as a plane.
//! Each sketch's face is placed ahead of its round's end (docs/ports/attacher.md, *Placed
//! ahead*), and a look ahead models and plans the whole part, so it's taken as often as the
//! kernel has room, not once a sketch (M5's milestone review): this grows as the part does.

use std::error::Error;
use std::io::Write as _;
use std::time::{Duration, Instant};

use silico_core::command;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::Op;
use silico_core::recompute::Recompute;
use silico_protocol::{Reply, Surface};

const LENGTHS: [usize; 4] = [500, 1000, 2000, 4000];

/// How many boxes the wide part has, each with its sketch and pad.
const WIDTHS: [usize; 3] = [100, 200, 400];

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

/// `k` boxes in a row, 20 mm apart, each with a square sketched on its top, padded onto it.
fn wide(k: usize) -> String {
    let mut features = Vec::new();
    for i in 0..k {
        let (b, s, p) = (id(3 * i), id(3 * i + 1), id(3 * i + 2));
        let x = 20 * i;
        features.push(format!(
            r#"{{"id": "{b}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": 10, "width": 10, "height": 10}}"#
        ));
        let corners = [(x + 1, 1), (x + 5, 1), (x + 5, 5), (x + 1, 5)];
        let points: Vec<String> = corners
            .iter()
            .enumerate()
            .map(|(n, (px, py))| {
                format!(
                    r#"{{"id": "{}", "x": {px}, "y": {py}, "construction": false}}"#,
                    id(1_000_000 + 4 * i + n)
                )
            })
            .collect();
        let lines: Vec<String> = (0..4)
            .map(|n| {
                format!(
                    r#"{{"id": "{}", "from": "{}", "to": "{}", "construction": false}}"#,
                    id(2_000_000 + 4 * i + n),
                    id(1_000_000 + 4 * i + n),
                    id(1_000_000 + 4 * i + (n + 1) % 4)
                )
            })
            .collect();
        features.push(format!(
            r#"{{"id": "{s}", "type": "sketch", "v": 1, "name": "Top", "plane": {{"face": "{b}.box.Face6", "x": "+x", "reversed": false}}, "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
            points.join(", "),
            lines.join(", ")
        ));
        features.push(format!(
            r#"{{"id": "{p}", "type": "pad", "v": 1, "name": "Boss", "base": "{b}", "sketch": "{s}", "extent": {{"type": "one_side", "length": 3, "reversed": false}}}}"#
        ));
    }
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
        features.join(",\n")
    )
}

/// An engine on a fake kernel of `size` threads that describes every face as the plane
/// z = 10, so each sketch on a box's top is placed and padded.
fn planar(size: usize) -> Result<Recompute<FakeKernel>> {
    let kernel = FakeKernel::new(size)?;
    kernel.answer_instead(|op| {
        let Op::DescribeAll { shape } = op else {
            return None;
        };
        let mut description = FakeKernel::described(shape)?;
        for face in &mut description.faces {
            face.surface = Surface::Plane {
                normal: [0.0, 0.0, 1.0],
                location: [0.0, 0.0, 10.0],
            };
        }
        Some(Reply::DescribedAll(description))
    });
    Ok(Recompute::new(kernel).with_sketches(FakeSketches::new()))
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
    writeln!(out)?;
    writeln!(out, "{:>6} {:>12} {:>12}", "boxes", "1 thread", "4 threads")?;
    for k in WIDTHS {
        let one = rebuilt(&planar(1)?, &wide(k))?;
        let four = rebuilt(&planar(4)?, &wide(k))?;
        writeln!(out, "{k:>6} {one:>12.1?} {four:>12.1?}")?;
    }
    Ok(())
}
