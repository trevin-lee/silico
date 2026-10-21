//! For tests that save a sketch and load it through the real sketch worker: a document's
//! geometry, the gate, the solve, the file a save writes, and what a solve says (included
//! with `#[path]`).
#![allow(dead_code)]

use std::path::Path;

use silico_core::document::{Checked, FeatureId, Operation, read, write};
use silico_core::sketch::{SketchSolution, SketchSolver, SketchWorker, solve};

pub const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
/// The sketch feature's ID.
pub const FEATURE: &str = "k7f2qx3m";

pub fn worker() -> SketchWorker {
    SketchWorker::start_worker(Path::new(WORKER)).unwrap()
}

pub fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

pub fn line(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
}

pub fn circle(id: &str, center: &str, r: f64) -> String {
    format!(r#"{{"id": "{id}", "center": "{center}", "radius": {r}, "construction": false}}"#)
}

pub fn arc(id: &str, center: &str, start: &str, end: &str) -> String {
    format!(
        r#"{{"id": "{id}", "center": "{center}", "start": "{start}", "end": "{end}", "construction": false}}"#
    )
}

/// `text` read and gated, or why not.
pub fn checked(text: &str) -> Result<Checked, String> {
    let doc = read(text.as_bytes()).map_err(|e| format!("unreadable: {e}"))?;
    Checked::new(doc).map_err(|e| format!("the gate refused: {:?}", e.errors()))
}

/// The sketch's solve, as a save or a load solves it.
pub fn solved(
    doc: &Checked,
    w: &mut dyn SketchSolver,
) -> Result<SketchSolution, silico_core::Error> {
    solve(doc, &FeatureId::new(FEATURE).unwrap(), w)
}

/// A whole number of nanometres, as the file holds dimensions.
pub fn nm(v: f64) -> f64 {
    (v * 1e6).round() / 1e6 + 0.0
}

/// `doc`'s sketch as a save writes it once `solution` is its solve (points, radii, and
/// reference values), through the file's text: the sketch a load solves from.
pub fn written_back(doc: &Checked, solution: &SketchSolution) -> Checked {
    let mut document = doc.document().clone();
    let Operation::Sketch(sketch) = &mut document.features[0].operation else {
        panic!("not a sketch")
    };
    *sketch = solution.written(sketch);
    checked(&write(&document).unwrap()).unwrap()
}

/// Where a solve put the sketch, with every coordinate and radius as bits, and the degrees
/// of freedom and free geometry it counts: what a load must give back unchanged. The
/// redundant constraints it names are compared apart, by each test.
pub fn said(s: &SketchSolution) -> String {
    let points: Vec<(String, u64, u64)> = s
        .points
        .iter()
        .map(|(id, [x, y])| (id.as_str().to_owned(), x.mm().to_bits(), y.mm().to_bits()))
        .collect();
    let radii: Vec<(String, u64)> = s
        .radii
        .iter()
        .map(|(id, r)| (id.as_str().to_owned(), r.mm().to_bits()))
        .collect();
    format!(
        "points {points:?} radii {radii:?} dofs {} free {:?}",
        s.dofs, s.free
    )
}
