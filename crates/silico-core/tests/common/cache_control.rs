//! For the tests of M4.3's cache control (`check`'s `fresh` and `cache_clear`): sessions on
//! a fake kernel and a fake sketch solver that the test keeps handles on, their documents in
//! a directory of their own, and the entries of the cache beside them, found as the cache
//! finds its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::command::{SessionId, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::sessions::answer;

/// An entry's name in a subdirectory: 62 lowercase hex digits.
pub const NAME: &str = "00000000000000000000000000000000000000000000000000000000000000";

/// Sessions on a fake kernel and a fake sketch solver, which the test keeps handles on,
/// each directory's cache sealed with a secret outside the documents' directory (`docs/`).
pub struct Fixture {
    pub dir: TempDir,
    pub kernel: Arc<FakeKernel>,
    pub sketches: FakeSketches,
    pub sessions: Sessions<Arc<FakeKernel>>,
}

pub fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    Fixture {
        dir,
        kernel,
        sketches,
        sessions,
    }
}

impl Fixture {
    /// The documents' directory.
    pub fn docs(&self) -> PathBuf {
        self.dir.path().join("docs")
    }

    /// `name` in the documents' directory.
    pub fn path(&self, name: &str) -> PathBuf {
        self.docs().join(name)
    }

    /// The cache beside the documents.
    pub fn cache(&self) -> PathBuf {
        self.path(".silico-cache")
    }

    /// `text` written to `docs/name` and opened.
    pub fn open(&self, name: &str, text: &str) -> SessionId {
        std::fs::create_dir_all(self.docs()).unwrap();
        let path = self.path(name);
        std::fs::write(&path, text).unwrap();
        let response = answer(self.sessions.open(&path, false));
        assert!(response.result.is_ok(), "{response:?}");
        response.session.unwrap().id
    }

    /// The entries in the cache beside the documents ([`entries`]).
    pub fn entries(&self) -> Vec<PathBuf> {
        entries(&self.cache())
    }
}

/// The entries in the cache `dir`, in order of path, found as the cache finds its own: a
/// regular file named by 62 lowercase hex digits in a subdirectory named by two, no link
/// followed.
pub fn entries(dir: &Path) -> Vec<PathBuf> {
    let hex = |name: &str, digits: usize| {
        name.len() == digits && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    };
    let mut found = Vec::new();
    for sub in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let named = sub.file_name().to_str().is_some_and(|name| hex(name, 2));
        if !named || !std::fs::symlink_metadata(sub.path()).unwrap().is_dir() {
            continue;
        }
        for file in std::fs::read_dir(sub.path()).unwrap().flatten() {
            let named = file.file_name().to_str().is_some_and(|name| hex(name, 62));
            if named && std::fs::symlink_metadata(file.path()).unwrap().is_file() {
                found.push(file.path());
            }
        }
    }
    found.sort();
    found
}
