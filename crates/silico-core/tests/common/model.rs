//! For tests that recompute: documents from text, and what a recompute published.

use std::sync::Arc;
use std::time::Duration;

use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::{Call, FakeKernel};
use silico_core::kernel::Kernel;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

/// Long enough for any recompute here, so one that hangs fails instead: a minute, since a
/// debug build of a test of thousands of broken references takes ten seconds on a loaded
/// machine.
pub const WAIT: Duration = Duration::from_secs(60);

pub fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

pub fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
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

/// The names of the kernel calls in `calls` that made shapes: every call but the summaries
/// and descriptions, which measure features' solids once they're made (checked in
/// tests/fingerprint.rs and tests/described.rs).
pub fn made(calls: &[Call]) -> Vec<&'static str> {
    calls
        .iter()
        .map(|call| call.op.name())
        .filter(|name| !matches!(*name, "Summarize" | "Describe"))
        .collect()
}

/// How the fake made the feature's solid.
pub fn described(model: &Model, feature: &str) -> String {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => FakeKernel::describe(&built.shape).unwrap(),
        other => panic!("{feature}: {other:?}"),
    }
}
