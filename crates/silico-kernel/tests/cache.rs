//! M2.5: the rebuild cache on OCCT. The pool's identity is its worker executable's: the
//! same executable is the same kernel, another one another kernel. A document built on the
//! pool is rebuilt from the cache by another session with no request to a worker, to the
//! same bytes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::cached;
use super::example;

use std::path::Path;
use std::sync::Arc;

use cached::{Counted, Scratch};
use silico_core::document::{Checked, read};
use silico_core::kernel::{Kernel, KernelRequest};
use silico_core::pool::KernelPool;
use silico_core::recompute::{Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

fn pool(exe: &str) -> KernelPool {
    KernelPool::start_on_demand(Path::new(exe), 1).unwrap()
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

fn published(engine: &Recompute<Counted>) -> Arc<Model> {
    let document = Checked::new(read(example::EXAMPLE.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

#[test]
fn a_document_built_on_occt_is_rebuilt_from_the_cache_to_the_same_bytes() {
    let dir = Scratch::new("occt");
    let session = || Recompute::with_cache(Counted::new(WORKER, 1), dir.cache());
    let first = session();
    let built = published(&first);
    // Four steps made, and three features' solids measured.
    assert_eq!(first.kernel().asked(), 7);
    drop(first);
    let second = session();
    let again = published(&second);
    assert_eq!(second.kernel().asked(), 0);
    assert_eq!(again.features(), built.features());
}

#[test]
fn the_worker_carries_the_occt_build_it_links_so_another_occt_is_another_kernel() {
    // OCCT is linked dynamically: its bytes aren't the worker's, so the build it links is
    // compiled into the worker, and the pool's identity (the worker's bytes) covers it.
    assert!(!silico_kernel::OCCT_BUILD.is_empty());
    let bytes = std::fs::read(WORKER).unwrap();
    let carries = |needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(
        carries(silico_kernel::OCCT_BUILD),
        "the worker doesn't carry {}",
        silico_kernel::OCCT_BUILD
    );
}

#[test]
fn the_worker_loads_the_occt_build_its_identity_names_not_the_checkouts_link() {
    // The build is the cache entry named by the hash of OCCT's inputs (64 hex digits), not
    // the checkout's link to it, which `pixi run occt` points at another build without the
    // worker changing; and the worker's run path is that entry's lib, so it can't load
    // another OCCT under the same bytes.
    let build = std::path::Path::new(silico_kernel::OCCT_BUILD);
    assert_eq!(build.file_name().unwrap(), "install");
    let entry = build
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        entry.len() == 64 && entry.bytes().all(|b| b.is_ascii_hexdigit()),
        "{} isn't a build cache entry",
        build.display()
    );
    let bytes = std::fs::read(WORKER).unwrap();
    let carries = |needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(carries(&format!("{}/lib", build.display())));
    assert!(
        !carries("target/occt/install"),
        "the worker still finds OCCT through the checkout's link"
    );
}

/// An echo on `pool` once its workers were killed: a request that reaches a killed worker
/// before the pool sees it die reports the kill (on Linux, often), so it's asked again, of
/// the worker that replaces it.
fn echo_after_kill(pool: &KernelPool) -> Result<Vec<u8>, silico_core::Error> {
    for _ in 0..3 {
        let request = KernelRequest::echo(silico_core::kernel::Blob::inline(vec![1]));
        let reply = pool
            .submit(pool.generation(), request)
            .wait_timeout(std::time::Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no outcome"))
            .done()
            .expect("superseded");
        match reply {
            Err(e) if e.cause().is_some_and(|cause| cause.contains("SIGKILL")) => continue,
            reply => return reply.map(|blob| blob.as_bytes().to_vec()),
        }
    }
    panic!("every echo reached a killed worker")
}

#[test]
fn a_worker_rebuilt_while_the_pool_runs_isnt_started_under_its_identity() {
    // A pool of a copy of the worker; the copy then replaced by another build (the test
    // worker), and the worker killed: its replacement would be the other build.
    let temp = tempfile::Builder::new()
        .prefix("silico-cache-rebuilt-")
        .tempdir()
        .unwrap();
    let dir = temp.path();
    let exe = dir.join("silico-kernel-worker");
    std::fs::copy(WORKER, &exe).unwrap();
    // Started now: its running worker is killed, and replaced from the changed file.
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
    let error = echo_after_kill(&pool).unwrap_err();
    assert_eq!(
        error.code(),
        silico_core::ErrorCode::WorkerUnavailable,
        "{error}"
    );
    assert!(
        error.message().contains("changed since it started"),
        "{error}"
    );
    assert_eq!(pool.identity(), identity);
    drop(pool);
}

#[test]
fn a_worker_reinstalled_with_the_same_bytes_is_still_started() {
    // The same executable copied over itself (a reinstall): a new file, so its size, time,
    // or inode may differ, but its bytes, and so its identity, don't.
    let temp = tempfile::Builder::new()
        .prefix("silico-cache-reinstalled-")
        .tempdir()
        .unwrap();
    let dir = temp.path();
    let exe = dir.join("silico-kernel-worker");
    std::fs::copy(WORKER, &exe).unwrap();
    // Started now: its running worker is killed, and replaced from the changed file.
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    let again = dir.join("again");
    std::fs::copy(WORKER, &again).unwrap();
    std::fs::rename(&again, &exe).unwrap();
    for pid in pool.pids() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    let reply = echo_after_kill(&pool);
    assert!(reply.is_ok(), "{:?}", reply.err());
    drop(pool);
}
