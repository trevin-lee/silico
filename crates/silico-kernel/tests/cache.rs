//! M2.5: the rebuild cache on OCCT. The pool's identity is its worker executable's: the
//! same executable is the same kernel, another one another kernel. A document built on the
//! pool is rebuilt from the cache by another session with no request to a worker, to the
//! same bytes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/example.rs"]
mod example;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use silico_core::cache::Cache;
use silico_core::document::{Checked, read};
use silico_core::kernel::{Generation, Kernel, KernelRequest};
use silico_core::pool::{KernelPool, Pending};
use silico_core::recompute::{Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

fn pool(exe: &str) -> KernelPool {
    KernelPool::start_workers(Path::new(exe), 1).unwrap()
}

#[test]
fn the_pools_identity_is_its_worker_executables() {
    let one = pool(WORKER);
    let again = pool(WORKER);
    let other = pool(TEST_WORKER);
    assert!(one.identity().starts_with("kernel worker sha256:"));
    assert_eq!(one.identity(), again.identity());
    assert_ne!(one.identity(), other.identity());
}

/// The pool, counting what it's asked.
struct Counted(KernelPool, AtomicUsize);

impl Kernel for Counted {
    fn identity(&self) -> &str {
        self.0.identity()
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
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.submit(generation, request)
    }
}

fn published(engine: &Recompute<Counted>) -> Arc<Model> {
    let document = Checked::new(read(example::EXAMPLE.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

#[test]
fn a_document_built_on_occt_is_rebuilt_from_the_cache_to_the_same_bytes() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("silico-cache-occt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let session = || {
        Recompute::with_cache(
            Counted(pool(WORKER), AtomicUsize::new(0)),
            Cache::open(&dir),
        )
    };
    let first = session();
    let built = published(&first);
    assert_eq!(first.kernel().1.load(Ordering::SeqCst), 4);
    drop(first);
    let second = session();
    let again = published(&second);
    assert_eq!(second.kernel().1.load(Ordering::SeqCst), 0);
    assert_eq!(again.features(), built.features());
    let _ = std::fs::remove_dir_all(&dir);
}
