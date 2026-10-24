//! M2.6: a feature's solid whose measure the kernel refuses for its values (the measure
//! would be refused again) is kept with that refusal, in memory and in the rebuild cache, so
//! it's asked once, not on every recompute and every open; one a worker failed to measure
//! (a crash, a hang, a shortage) is asked again. On disk only invalid input and an invalid
//! result are kept: `kernel_failed` may come of a shortage of memory OCCT caught itself.
//! (The format, and entries of the format before, are tested in adversarial_cache.rs.)
//! Expected values are written by hand: which kernel calls each recompute makes, and the
//! refusals the fake gives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::cache;
use super::example;
use super::model;
use super::refusals;

use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, session};
use example::EXAMPLE;
use model::{checked, published};
use refusals::refusing;
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::recompute::Recompute;
use silico_core::{Error, ErrorCode};

/// The example's three features, each refused its measure with `code`, as `drift` says it.
fn refused(code: ErrorCode) -> Vec<(ErrorCode, String)> {
    ["k7f2qx3m", "a91cw0de", "m3tq8v0z"]
        .into_iter()
        .map(|id| (code, id.to_owned()))
        .collect()
}

/// What a model says of its features' measures: each error's code and feature.
fn said(errors: &[Error]) -> Vec<(ErrorCode, String)> {
    errors
        .iter()
        .map(|e| (e.code(), e.feature().unwrap_or_default().to_owned()))
        .collect()
}

/// How many measures a kernel was asked for.
fn measures(kernel: &FakeKernel) -> usize {
    kernel
        .calls()
        .iter()
        .filter(|call| matches!(call.op, Op::Summarize { .. }))
        .count()
}

#[test]
fn a_measure_refused_for_its_values_is_asked_once_in_a_session() {
    for code in [
        ErrorCode::KernelFailed,
        ErrorCode::InvalidResult,
        ErrorCode::InvalidInput,
    ] {
        let engine = Recompute::new(FakeKernel::new(2).unwrap());
        refusing(engine.kernel(), code);
        let first = published(&engine, checked(EXAMPLE));
        assert_eq!(measures(engine.kernel()), 3, "{code:?}");
        assert_eq!(said(&first.drift()), refused(code), "{code:?}");
        // Again, unchanged: nothing is asked of the kernel, and the model says the same, to
        // the error.
        let asked = engine.kernel().calls().len();
        let again = published(&engine, checked(EXAMPLE));
        assert_eq!(engine.kernel().calls().len(), asked, "{code:?}");
        assert_eq!(again.drift(), first.drift(), "{code:?}");
        // Every feature still built.
        assert_eq!(hole(&again), HOLE);
        // An edit of the pin asks for the pin's and the cut's measures; the block's
        // refusal stands.
        published(&engine, example_with_pin("3"));
        assert_eq!(measures(engine.kernel()), 5, "{code:?}");
    }
}

#[test]
fn a_measure_refused_as_invalid_is_kept_on_disk_and_never_asked_again() {
    // Invalid input or an invalid result: facts about the solid. (`kernel_failed` is kept
    // only for the session: OCCT's integration may come back so for a shortage of memory.)
    for code in [ErrorCode::InvalidInput, ErrorCode::InvalidResult] {
        let dir = Scratch::new(&format!("refused-kept-{}", code.as_str()));
        let first = session(&dir.0);
        refusing(first.kernel(), code);
        let model = published(&first, checked(EXAMPLE));
        assert_eq!(said(&model.drift()), refused(code));
        // A session that would measure them: it asks the kernel nothing, and says the same.
        let second = session(&dir.0);
        let again = published(&second, checked(EXAMPLE));
        assert!(second.kernel().calls().is_empty(), "{:?}", calls(&second));
        assert_eq!(again.drift(), model.drift());
        assert_eq!(hole(&again), HOLE);
        // Nor is anything written again: the entries are as the first session left them.
        let before: Vec<Vec<u8>> = files(&dir.0)
            .iter()
            .map(|f| std::fs::read(f).unwrap())
            .collect();
        let third = session(&dir.0);
        published(&third, checked(EXAMPLE));
        assert!(third.kernel().calls().is_empty());
        let after: Vec<Vec<u8>> = files(&dir.0)
            .iter()
            .map(|f| std::fs::read(f).unwrap())
            .collect();
        assert_eq!(after, before);
    }
}

#[test]
fn a_measure_a_worker_failed_is_asked_again() {
    // A crash, a hang, a shortage, a reply that couldn't be read, or a bug: none follows
    // from the solid, so each recompute asks again, and the cache keeps the solid
    // unmeasured.
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::ResourceExhausted,
        ErrorCode::ProtocolError,
        ErrorCode::Internal,
    ] {
        let engine = Recompute::new(FakeKernel::new(2).unwrap());
        refusing(engine.kernel(), code);
        let first = published(&engine, checked(EXAMPLE));
        assert_eq!(said(&first.drift()), refused(code), "{code:?}");
        published(&engine, checked(EXAMPLE));
        assert_eq!(measures(engine.kernel()), 6, "{code:?}");
        // And from the disk: the next session measures them.
        let dir = Scratch::new(&format!("failed-{}", code.as_str()));
        let failing = session(&dir.0);
        refusing(failing.kernel(), code);
        published(&failing, checked(EXAMPLE));
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert_eq!(measures(next.kernel()), 3, "{code:?}");
        assert!(model.drift().is_empty(), "{code:?}");
    }
}
