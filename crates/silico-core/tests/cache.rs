//! M2.5: the rebuild cache. A session that opens a document another session built takes
//! each step's result from the cache, not the kernel; an edit misses only for the steps it
//! changes; another kernel or platform misses everything; an entry that doesn't hold
//! together is rebuilt and written over, never trusted; failures aren't kept on disk; and a
//! cache that can't be written to fails nothing. Expected values are written by hand: which
//! kernel calls each session makes, and the shapes, as the fake describes what made them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use example::EXAMPLE;
use silico_core::cache::Cache;
use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Generation, Kernel, KernelRequest, Op, Primitive};
use silico_core::pool::Pending;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

/// A directory of its own for one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("silico-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn checked(text: &str) -> Checked {
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

/// The example with the pin's radius `radius`.
fn example_with_pin(radius: &str) -> Checked {
    checked(&EXAMPLE.replace(r#""radius": 2.5"#, &format!(r#""radius": {radius}"#)))
}

fn published<K: Kernel + 'static>(engine: &Recompute<K>, document: Checked) -> Arc<Model> {
    match engine
        .start(document)
        .wait_timeout(std::time::Duration::from_secs(10))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("not published: {:?}", other.map(|r| r.map(|_| ())).ok()),
    }
}

/// A fresh session: a new fake kernel, over the cache in `dir`.
fn session(dir: &Path) -> Recompute<FakeKernel> {
    Recompute::with_cache(FakeKernel::new(2).unwrap(), Cache::open(dir))
}

fn calls(engine: &Recompute<FakeKernel>) -> Vec<&'static str> {
    let mut calls: Vec<_> = engine
        .kernel()
        .calls()
        .iter()
        .map(|c| c.op.name())
        .collect();
    calls.sort_unstable();
    calls
}

fn hole(model: &Model) -> String {
    match model.outcome(&FeatureId::new("m3tq8v0z").unwrap()) {
        Some(FeatureOutcome::Built { built, .. }) => FakeKernel::describe(&built.shape).unwrap(),
        other => panic!("{other:?}"),
    }
}

/// The cache's entries' files, in order.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for sub in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(sub.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            files.push(file.path());
        }
    }
    files.sort();
    files
}

#[test]
fn a_document_built_once_is_rebuilt_from_the_cache_with_no_kernel_call() {
    let dir = Scratch::new("hit");
    let first = session(&dir.0);
    let built = published(&first, checked(EXAMPLE));
    assert_eq!(
        calls(&first),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    // One entry per step.
    assert_eq!(entries(&dir.0).len(), 4);
    drop(first);
    let second = session(&dir.0);
    let again = published(&second, checked(EXAMPLE));
    assert!(calls(&second).is_empty(), "{:?}", calls(&second));
    // Identical geometry: the same shapes, histories, and validity, feature for feature.
    assert_eq!(again.features(), built.features());
    assert_eq!(
        hole(&again),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
}

#[test]
fn an_edit_misses_only_for_the_steps_it_changes() {
    let dir = Scratch::new("edit");
    published(&session(&dir.0), checked(EXAMPLE));
    let second = session(&dir.0);
    let model = published(&second, example_with_pin("3"));
    // The block's make is kept; the pin's make, its move, and the cut are new.
    assert_eq!(calls(&second), ["Cut", "MakeCylinder", "Translate"]);
    assert_eq!(
        hole(&model),
        "cut(box(40, 20, 10), translate(cylinder(3, 12), 20, 10, -1))"
    );
    // And both versions are now kept.
    let third = session(&dir.0);
    published(&third, checked(EXAMPLE));
    published(&third, example_with_pin("3"));
    assert!(calls(&third).is_empty());
}

/// The fake kernel under another identity: another build of the kernel.
struct Rebuilt(FakeKernel);

impl Kernel for Rebuilt {
    fn identity(&self) -> &str {
        "silico fake kernel 2"
    }

    fn generation(&self) -> Generation {
        self.0.generation()
    }

    fn advance(&self, generation: Generation) {
        self.0.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.0.submit(generation, request)
    }
}

#[test]
fn another_kernel_misses_everything() {
    let dir = Scratch::new("kernel");
    published(&session(&dir.0), checked(EXAMPLE));
    let rebuilt = Recompute::with_cache(Rebuilt(FakeKernel::new(2).unwrap()), Cache::open(&dir.0));
    published(&rebuilt, checked(EXAMPLE));
    assert_eq!(rebuilt.kernel().0.calls().len(), 4);
}

#[test]
fn another_platform_misses_everything() {
    let dir = Scratch::new("platform");
    published(&session(&dir.0), checked(EXAMPLE));
    let elsewhere = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::for_platform(&dir.0, "elsewhere-riscv64"),
    );
    published(&elsewhere, checked(EXAMPLE));
    assert_eq!(
        calls(&elsewhere),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    // Both platforms' entries are kept, side by side.
    assert_eq!(entries(&dir.0).len(), 8);
}

/// Damages an entry's bytes, given another entry's.
type Damage = fn(&[u8], &[u8]) -> Vec<u8>;

#[test]
fn an_entry_that_doesnt_hold_together_is_rebuilt_and_written_over() {
    // Each way an entry can be damaged: a byte flipped in its payload, in its checksum, or
    // in its magic; cut short; empty; another entry's bytes under its name.
    let damages: [(&str, Damage); 6] = [
        ("payload", |bytes, _| {
            let mut bytes = bytes.to_vec();
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            bytes
        }),
        ("checksum", |bytes, _| {
            let mut bytes = bytes.to_vec();
            bytes[20] ^= 1;
            bytes
        }),
        ("magic", |bytes, _| {
            let mut bytes = bytes.to_vec();
            bytes[0] ^= 1;
            bytes
        }),
        ("short", |bytes, _| bytes[..bytes.len() / 2].to_vec()),
        ("empty", |_, _| Vec::new()),
        ("another", |_, other| other.to_vec()),
    ];
    for (damage, damaged) in damages {
        let dir = Scratch::new(&format!("corrupt-{damage}"));
        published(&session(&dir.0), checked(EXAMPLE));
        let files = entries(&dir.0);
        let (victim, other) = (&files[0], &files[1]);
        let sound = std::fs::read(victim).unwrap();
        std::fs::write(victim, damaged(&sound, &std::fs::read(other).unwrap())).unwrap();
        let second = session(&dir.0);
        let model = published(&second, checked(EXAMPLE));
        // One step rebuilt, the rest kept; the same result.
        assert_eq!(second.kernel().calls().len(), 1, "{damage}");
        assert_eq!(
            hole(&model),
            "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))",
            "{damage}"
        );
        // Written over, sound again.
        assert_eq!(std::fs::read(victim).unwrap(), sound, "{damage}");
    }
}

#[test]
fn failures_arent_kept_on_disk() {
    let dir = Scratch::new("failure");
    let first = session(&dir.0);
    first.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    published(&first, checked(EXAMPLE));
    // The block's make only: the pin failed, and the cut was blocked.
    assert_eq!(entries(&dir.0).len(), 1);
    // Another session asks the kernel again, and builds.
    let second = session(&dir.0);
    let model = published(&second, checked(EXAMPLE));
    assert_eq!(calls(&second), ["Cut", "MakeCylinder", "Translate"]);
    assert_eq!(
        hole(&model),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
}

#[test]
fn a_cache_that_cant_be_written_fails_nothing() {
    // A file where the cache's directory would be: nothing can be written under it.
    let dir = Scratch::new("unwritable");
    std::fs::write(&dir.0, b"not a directory").unwrap();
    let engine = session(&dir.0);
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(
        hole(&model),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    let _ = std::fs::remove_file(&dir.0);
}

#[test]
fn a_hit_is_a_shape_the_worker_checked() {
    let dir = Scratch::new("checked");
    published(&session(&dir.0), checked(EXAMPLE));
    let model = published(&session(&dir.0), checked(EXAMPLE));
    for (_, outcome) in model.features() {
        let FeatureOutcome::Built { built, .. } = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(built.shape.valid(), Some(true));
    }
}
