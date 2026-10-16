//! For recompute's tests: documents from text, an engine on the fake kernel, and what it
//! published and asked.

use std::sync::Arc;
use std::time::Duration;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Kernel;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

/// Long enough for any recompute here, so one that hangs fails instead.
pub const WAIT: Duration = Duration::from_secs(10);

pub fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

/// A document of these features, each a JSON object's text.
pub fn document(features: &[&str]) -> Checked {
    checked(&format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    ))
}

pub fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

pub fn engine(size: usize) -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(size).unwrap())
}

/// Recomputes `document` and returns the model it published, failing if it was superseded
/// or didn't finish within [`WAIT`].
pub fn published<K: Kernel + 'static>(engine: &Recompute<K>, document: Checked) -> Arc<Model> {
    match engine.start(document).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in {WAIT:?}"),
    }
}

/// The names of the kernel calls made since `from`.
pub fn calls_since(engine: &Recompute<FakeKernel>, from: usize) -> Vec<&'static str> {
    engine.kernel().calls()[from..]
        .iter()
        .map(|call| call.op.name())
        .collect()
}

/// How the fake made the feature's solid.
pub fn described(model: &Model, feature: &str) -> String {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => FakeKernel::describe(&built.shape).unwrap(),
        other => panic!("{feature}: {other:?}"),
    }
}
