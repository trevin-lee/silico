//! For tests that recompute: documents from text, and what a recompute published. Shared
//! with silico-kernel's tests, which use only some of it.
#![allow(dead_code)]

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

/// `f`'s result, if it returns within [`WAIT`]: for something that should be prompt (a
/// drop, a close) and would, wrong, block on a request held until the test lets it go,
/// which a test must fail at, not hang on. Run on a thread of its own, left blocked if it
/// never returns.
pub fn promptly<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(WAIT) {
        Ok(out) => out,
        // `f` panicked: its panic, not a timeout.
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("{what} ended without a result"),
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("{what} didn't return in {WAIT:?}")
        }
    }
}

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
