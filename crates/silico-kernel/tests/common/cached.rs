//! For the cache's tests on OCCT: a pool that counts what it's asked, and scratch caches
//! with secrets of their own.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use silico_core::cache::Cache;
use silico_core::kernel::{Generation, Kernel, KernelRequest};
use silico_core::pool::{KernelPool, Pending};

/// The pool, counting what it's asked.
pub struct Counted(pub KernelPool, AtomicUsize);

impl Counted {
    /// A pool of `workers` of the worker at `exe`.
    pub fn new(exe: &str, workers: usize) -> Self {
        Self(
            KernelPool::start_workers(Path::new(exe), workers).unwrap(),
            AtomicUsize::new(0),
        )
    }

    /// How many requests it's been asked.
    pub fn asked(&self) -> usize {
        self.1.load(Ordering::SeqCst)
    }
}

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

/// A cache directory of its own for one test, and its secret's file beside it, both removed
/// when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let scratch = Self(std::env::temp_dir().join(format!(
            "silico-{}-{name}-{}",
            env!("CARGO_CRATE_NAME"),
            std::process::id()
        )));
        scratch.remove();
        scratch
    }

    /// The cache in this directory, for this platform, with its own secret.
    pub fn cache(&self) -> Cache {
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        Cache::for_platform(&self.0, &platform, &self.secret())
    }

    fn secret(&self) -> PathBuf {
        let mut name = self.0.as_os_str().to_owned();
        name.push(".secret");
        PathBuf::from(name)
    }

    fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = std::fs::remove_file(self.secret());
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove();
    }
}
