//! For the tests of descriptions made in the rebuild: what a fake kernel was asked, and
//! documents of solids too large for one batch of a kind.

use std::time::{Duration, Instant};

use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;

use super::model::WAIT;

/// How many requests of `op` a kernel was asked.
pub fn asked(kernel: &FakeKernel, op: &str) -> usize {
    asked_of(kernel, |asked| asked.name() == op)
}

/// How many requests `picks` a kernel was asked.
pub fn asked_of(kernel: &FakeKernel, picks: impl Fn(&Op) -> bool) -> usize {
    kernel.calls().iter().filter(|call| picks(&call.op)).count()
}

/// How many requests `picks` a kernel has started once it has started `n` of them, and a
/// little longer, failing, with how many started, after [`WAIT`]: `n`, unless it started
/// more. A description that fails cancels its other batches in flight, but one a worker had
/// already taken runs on, unwaited, so it may start after the model is published. The wait
/// is the binary's own for a run, not a shorter one: a request held mid-run starts when the
/// run reaches it, which a loaded machine delays.
pub fn settled(kernel: &FakeKernel, picks: impl Fn(&Op) -> bool, n: usize) -> usize {
    let deadline = Instant::now() + WAIT;
    while asked_of(kernel, &picks) < n {
        assert!(
            Instant::now() < deadline,
            "{} of {n} requests started",
            asked_of(kernel, &picks)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(50));
    asked_of(kernel, &picks)
}

/// A document of `n` boxes 20 apart along x, each fused onto the fusion of those before:
/// fusion `f{i}` holds `i + 1` boxes' elements (the fake keeps every box's).
pub fn fused_boxes(n: usize) -> String {
    let mut features = Vec::new();
    for i in 0..n {
        features.push(format!(
            r#"    {{"id": "b{i:07}", "type": "box", "v": 1, "name": "Box", "position": [{}, 0, 0], "length": 10, "width": 10, "height": 10}}"#,
            i * 20
        ));
    }
    for i in 1..n {
        let base = if i == 1 {
            "b0000000".to_owned()
        } else {
            format!("f{:07}", i - 1)
        };
        features.push(format!(
            r#"    {{"id": "f{i:07}", "type": "fuse", "v": 1, "name": "Fuse", "base": "{base}", "tool": "b{i:07}"}}"#
        ));
    }
    document(&features)
}

/// A document of `2^levels` boxes 20 apart along x, fused pairwise, then those fusions
/// pairwise, and so on, into one: its last fusion, `t{levels}0000`, holds every box's
/// elements, and each level's fusions hold `2^levels` boxes between them, so describing the
/// whole tree costs `levels + 1` times the boxes' elements, not a chain's square.
pub fn fused_tree(levels: u32) -> String {
    let n = 1usize << levels;
    let mut features = Vec::new();
    for i in 0..n {
        features.push(format!(
            r#"    {{"id": "t0{i:06}", "type": "box", "v": 1, "name": "Box", "position": [{}, 0, 0], "length": 10, "width": 10, "height": 10}}"#,
            i * 20
        ));
    }
    for level in 1..=levels {
        for i in 0..(n >> level) {
            features.push(format!(
                r#"    {{"id": "t{level}{i:06}", "type": "fuse", "v": 1, "name": "Fuse", "base": "t{}{:06}", "tool": "t{}{:06}"}}"#,
                level - 1,
                2 * i,
                level - 1,
                2 * i + 1
            ));
        }
    }
    document(&features)
}

fn document(features: &[String]) -> String {
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 6,\n  \"features\": [\n{}\n  ]\n}}\n",
        features.join(",\n")
    )
}
