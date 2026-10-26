//! For the cache's tests: scratch caches with secrets of their own, sessions on the fake
//! kernel over them, and what a session asked and built.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use silico_core::cache::Cache;
use silico_core::document::Checked;
use silico_core::fake::FakeKernel;
use silico_core::recompute::{Model, Recompute};

use crate::example::EXAMPLE;
use crate::model::{checked, described, made};

/// The example's hole, as the fake describes what made it.
pub const HOLE: &str = "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))";

/// A cache directory of its own for one test, and its secret's file beside it (not in it,
/// as a user's is in their home), both removed when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "silico-{}-{name}-{}",
            env!("CARGO_CRATE_NAME"),
            std::process::id()
        ));
        let scratch = Self(dir);
        scratch.remove();
        scratch
    }

    /// The file this cache's secret is in.
    pub fn secret(&self) -> PathBuf {
        secret_of(&self.0)
    }

    fn remove(&self) {
        let _ = make_writable(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.secret());
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove();
    }
}

fn make_writable(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o755));
        let _ = make_writable(&entry.path());
    }
    Ok(())
}

/// The secret's file for the cache in `dir`.
pub fn secret_of(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(".secret");
    PathBuf::from(name)
}

/// The cache in `dir`, for this platform, with its own secret.
pub fn cache(dir: &Path) -> Cache {
    Cache::with_secret(dir, &secret_of(dir))
}

/// A fresh session: a new fake kernel, over the cache in `dir`.
pub fn session(dir: &Path) -> Recompute<FakeKernel> {
    Recompute::with_cache(FakeKernel::new(2).unwrap(), cache(dir))
}

/// The example with the pin's radius `radius`.
pub fn example_with_pin(radius: &str) -> Checked {
    checked(&EXAMPLE.replace(r#""radius": 2.5"#, &format!(r#""radius": {radius}"#)))
}

/// The names of the kernel calls a session made shapes with (no summaries, [`made`]), in
/// order of name.
pub fn calls(engine: &Recompute<FakeKernel>) -> Vec<&'static str> {
    let mut calls = made(&engine.kernel().calls());
    calls.sort_unstable();
    calls
}

/// How the fake made the example's hole.
pub fn hole(model: &Model) -> String {
    described(model, "m3tq8v0z")
}

/// Every file in the cache's subdirectories, in order: its entries, and any partial files
/// and stray files beside them (`cache_control::entries` lists only what the cache counts as
/// an entry).
pub fn entries_and_partials(dir: &Path) -> Vec<PathBuf> {
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
