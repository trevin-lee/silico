//! For recompute's tests: documents of features, an engine on the fake kernel, and what it
//! asked.

use silico_core::document::Checked;
use silico_core::fake::FakeKernel;
use silico_core::recompute::Recompute;

use crate::model::checked;

/// A document of these features, each a JSON object's text.
pub fn document(features: &[&str]) -> Checked {
    checked(&format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    ))
}

pub fn engine(size: usize) -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(size).unwrap())
}

/// The names of the kernel calls made since `from`.
pub fn calls_since(engine: &Recompute<FakeKernel>, from: usize) -> Vec<&'static str> {
    engine.kernel().calls()[from..]
        .iter()
        .map(|call| call.op.name())
        .collect()
}
