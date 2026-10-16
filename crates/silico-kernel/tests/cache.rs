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

#[test]
fn the_worker_carries_the_occt_build_it_links_so_another_occt_is_another_kernel() {
    // OCCT is linked dynamically: its bytes aren't the worker's, so the build it links is
    // compiled into the worker, and the pool's identity (the worker's bytes) covers it.
    assert!(!silico_kernel::OCCT_BUILD.is_empty());
    let bytes = std::fs::read(WORKER).unwrap();
    let needle = silico_kernel::OCCT_BUILD.as_bytes();
    assert!(
        bytes.windows(needle.len()).any(|w| w == needle),
        "the worker doesn't carry {}",
        silico_kernel::OCCT_BUILD
    );
}

#[test]
fn a_worker_rebuilt_while_the_pool_runs_isnt_started_under_its_identity() {
    // A pool of a copy of the worker; the copy then replaced by another build (the test
    // worker), and the worker killed: its replacement would be the other build.
    let dir = std::env::temp_dir().join(format!("silico-cache-rebuilt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("silico-kernel-worker");
    std::fs::copy(WORKER, &exe).unwrap();
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    let identity = pool.identity().to_owned();
    // Replaced, not rewritten in place: a new file under the name, as a rebuild leaves it.
    let other = dir.join("other");
    std::fs::copy(TEST_WORKER, &other).unwrap();
    std::fs::rename(&other, &exe).unwrap();
    for pid in pool.pids() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    let request = KernelRequest::echo(silico_core::kernel::Blob::inline(vec![1]));
    let error = pool
        .submit(pool.generation(), request)
        .wait_timeout(std::time::Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no outcome"))
        .done()
        .expect("superseded")
        .unwrap_err();
    assert!(
        error.message().contains("changed since the pool started"),
        "{error}"
    );
    assert_eq!(pool.identity(), identity);
    drop(pool);
    let _ = std::fs::remove_dir_all(&dir);
}
