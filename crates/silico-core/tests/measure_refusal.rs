//! M2.6: a feature's solid whose measure the kernel refuses for its values (the measure
//! would be refused again) is kept with that refusal, in memory and in the rebuild cache, so
//! it's asked once, not on every recompute and every open; one a worker failed to measure
//! (a crash, a hang, a shortage) is asked again. And an entry of the cache's format before
//! this one is a miss, not a failure, and is written over in this one. Expected values are
//! written by hand: which kernel calls each recompute makes, and the refusals the fake gives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/cache.rs"]
mod cache;
#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;

use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, session};
use example::EXAMPLE;
use model::{checked, published};
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

/// A kernel that refuses every measure with `code`.
fn refusing(kernel: &FakeKernel, code: ErrorCode) {
    kernel.refuse(move |op| {
        matches!(op, Op::Summarize { .. }).then(|| Error::new(code, "Summarize failed"))
    });
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
fn a_measure_refused_for_its_values_is_kept_on_disk_and_never_asked_again() {
    let dir = Scratch::new("refused-kept");
    let first = session(&dir.0);
    refusing(first.kernel(), ErrorCode::KernelFailed);
    let model = published(&first, checked(EXAMPLE));
    assert_eq!(said(&model.drift()), refused(ErrorCode::KernelFailed));
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

#[test]
fn an_entry_of_the_format_before_is_a_miss_and_written_over_in_this_one() {
    // Entries of the format before this one ("silico cache 4") are keyed under it, so this
    // format never looks for them: a planted one is a miss, left as it was (the cache's
    // bound evicts it in time), and the step is built and written in this format.
    let dir = Scratch::new("format-before");
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    let old = dir.0.join("ab").join("0".repeat(62));
    let mut bytes = b"silico cache 4\n".to_vec();
    bytes.extend([7; 32]);
    bytes.extend(b"a payload of the format before");
    std::fs::write(&old, &bytes).unwrap();
    let first = session(&dir.0);
    let model = published(&first, checked(EXAMPLE));
    assert_eq!(
        calls(&first),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    assert!(model.drift().is_empty());
    assert_eq!(std::fs::read(&old).unwrap(), bytes);
    let written: Vec<_> = files(&dir.0).into_iter().filter(|f| *f != old).collect();
    assert_eq!(written.len(), 4);
    for file in &written {
        assert!(
            std::fs::read(file)
                .unwrap()
                .starts_with(b"silico cache 5\n"),
            "{file:?}"
        );
    }
    // An entry of this format's name with the format before's bytes doesn't hold together:
    // rebuilt, and written over in this format.
    let entry = &written[0];
    let mut text = std::fs::read(entry).unwrap();
    text[..15].copy_from_slice(b"silico cache 4\n");
    std::fs::write(entry, &text).unwrap();
    let second = session(&dir.0);
    let model = published(&second, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    assert_eq!(calls(&second).len(), 1, "{:?}", calls(&second));
    assert!(
        std::fs::read(entry)
            .unwrap()
            .starts_with(b"silico cache 5\n")
    );
}
