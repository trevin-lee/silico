//! The rebuild cache (`.silico-cache/`, ARCHITECTURE.md): each kernel step's result kept on
//! disk, so a document opened again, or by another session, rebuilds without the kernel. An
//! entry is a step's shape (its B-rep and the worker's word on its validity) and history,
//! under one checksum, keyed by the step's recompute key, the kernel's identity, and the
//! platform: a result is used only by the same kernel, for the same request, on the same
//! kind of machine, and a hit is a shape whose validity the worker has already checked.
//!
//! Only results are kept, never failures: a failure's cause may not last (memory, say), and
//! recompute keeps those that follow from the values in memory. Nothing here fails a
//! recompute: an entry that can't be read is a miss, and one that can't be written is left
//! unwritten, each with a warning. An entry that doesn't hold together (a bad checksum, a
//! payload that doesn't parse, an entry for another key) is a miss too, rebuilt and written
//! over, never trusted.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::kernel::{Blob, History, ShapeData};
use crate::recompute::{Built, Key};

/// The first bytes of every entry, naming its format: a change to the format changes it.
const MAGIC: &[u8] = b"silico cache 1\n";

/// A directory of kept results, for one platform.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    platform: String,
}

/// An entry's payload: what the checksum covers.
#[derive(Serialize, Deserialize)]
struct Entry {
    /// The entry's own key, so an entry copied under another key's name isn't taken for it.
    key: [u8; 32],
    brep: Vec<u8>,
    valid: bool,
    history: Option<History>,
}

impl Cache {
    /// The cache in `dir` (`.silico-cache/` beside a document, say), for this platform.
    /// The directory is made when the first entry is written.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self::for_platform(dir, &this_platform())
    }

    /// The cache in `dir`, for `platform` (`macos-aarch64`, `linux-x86_64`): for tests and
    /// tools; a session uses [`open`](Self::open).
    pub fn for_platform(dir: impl Into<PathBuf>, platform: &str) -> Self {
        Self {
            dir: dir.into(),
            platform: platform.to_owned(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The result kept for `step` on `kernel` (its [`identity`](crate::kernel::Kernel::identity)),
    /// if there's a sound one.
    pub(crate) fn get(&self, kernel: &str, step: &Key) -> Option<Built> {
        let key = self.key(kernel, step);
        let path = self.path(&key);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(path = %path.display(), "couldn't read a cache entry: {e}");
                return None;
            }
        };
        match read_entry(&bytes, &key) {
            Some(entry) => Some(Built {
                shape: ShapeData::checked(Blob::inline(entry.brep), entry.valid),
                history: entry.history,
            }),
            None => {
                tracing::warn!(path = %path.display(), "a cache entry doesn't hold together: rebuilding it");
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Keeps `built`, the result of `step` on `kernel`, if the worker said whether its shape
    /// is valid (a shape it hasn't checked isn't kept, so a hit is always checked).
    pub(crate) fn put(&self, kernel: &str, step: &Key, built: &Built) {
        let Some(valid) = built.shape.valid() else {
            return;
        };
        let key = self.key(kernel, step);
        let entry = Entry {
            key,
            brep: built.shape.brep().as_bytes().to_vec(),
            valid,
            history: built.history.clone(),
        };
        let payload = match postcard::to_allocvec(&entry) {
            Ok(payload) => payload,
            Err(e) => {
                tracing::warn!("couldn't encode a cache entry: {e}");
                return;
            }
        };
        let path = self.path(&key);
        if let Err(e) = write_entry(&path, &payload) {
            tracing::warn!(path = %path.display(), "couldn't write a cache entry: {e}");
        }
    }

    /// An entry's key: SHA-256 of the format, the kernel, the platform, and the step.
    fn key(&self, kernel: &str, step: &Key) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(MAGIC);
        for part in [kernel, &self.platform, &step.to_string()] {
            hash.update(part.as_bytes());
            hash.update(b"\n");
        }
        hash.finalize().into()
    }

    /// Where an entry lives: under the first two hex digits of its key, so no directory
    /// holds every entry.
    fn path(&self, key: &[u8; 32]) -> PathBuf {
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        self.dir.join(&hex[..2]).join(&hex[2..])
    }
}

/// The platform's name: its OS and architecture (`macos-aarch64`).
fn this_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The entry `bytes` hold for `key`, if they hold together: the format's magic, a checksum
/// that matches the payload, a payload that parses, and the key it was written for.
fn read_entry(bytes: &[u8], key: &[u8; 32]) -> Option<Entry> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (sum, payload) = rest.split_at_checked(32)?;
    if Sha256::digest(payload).as_slice() != sum {
        return None;
    }
    let entry: Entry = postcard::from_bytes(payload).ok()?;
    (entry.key == *key).then_some(entry)
}

/// Writes an entry at `path` whole or not at all: to a file of its own beside it, then
/// renamed over it, so a reader never sees a part of one, and two writers of the same entry
/// (two sessions) leave one or the other.
fn write_entry(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no directory"))?;
    std::fs::create_dir_all(dir)?;
    let partial = dir.join(format!(
        ".partial-{}-{}",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = std::fs::File::create(&partial)?;
        file.write_all(MAGIC)?;
        file.write_all(&Sha256::digest(payload))?;
        file.write_all(payload)?;
        file.sync_all()
    })();
    let renamed = written.and_then(|()| std::fs::rename(&partial, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    renamed
}
