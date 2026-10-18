//! For the tests of references by name: the example's block, pin, and hole, a fillet of
//! them as a document writes it, and what the fake kernel was asked to dress. Documents are
//! read and recomputed with `common/model.rs`'s helpers.

use silico_core::fake::FakeKernel;
use silico_core::kernel::{ChamferSize, Op};
use silico_core::recompute::Recompute;

pub const HEAD: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;
pub const BLOCK: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
pub const PIN: &str = r#"{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12}"#;
pub const HOLE: &str = r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#;

pub fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap())
}

/// The example's block, pin, and hole, then `more`.
pub fn document(more: &[&str]) -> String {
    let mut features = vec![BLOCK, PIN, HOLE];
    features.extend(more);
    format!("{HEAD}{}]}}", features.join(",\n"))
}

/// `edges` as a JSON list's items.
pub fn names(edges: &[&str]) -> String {
    let edges: Vec<String> = edges
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect();
    edges.join(", ")
}

pub fn fillet(id: &str, base: &str, edges: &[&str], radius: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": [{}], "radius": {radius}}}"#,
        names(edges)
    )
}

/// The fillets and chamfers the kernel was asked for: (edges, size).
pub fn dressings(kernel: &FakeKernel) -> Vec<(Vec<u32>, f64)> {
    kernel
        .calls()
        .into_iter()
        .filter_map(|call| match call.op {
            Op::Fillet { edges, radius, .. } => Some((edges, radius)),
            Op::Chamfer {
                edges,
                size: ChamferSize::Equal { distance },
                ..
            } => Some((edges, distance)),
            _ => None,
        })
        .collect()
}
