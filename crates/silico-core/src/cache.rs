//! The rebuild cache (`.silico-cache/`, ARCHITECTURE.md): each kernel step's result kept on
//! disk, so a document opened again, or by another session, rebuilds without the kernel. An
//! entry is a step's shape (its B-rep and the worker's word on its validity), history, and
//! summary, under one seal, keyed by the step's recompute key, the kernel's identity, and the
//! platform: a result is used only by the same kernel, for the same request, on the same
//! kind of machine, and a hit is a shape whose validity the worker has already checked.
//!
//! The seal is an HMAC-SHA-256 under a secret of the user's, kept outside any document (in
//! Silico's directory in the user's cache), so an entry is taken only if this user's Silico
//! wrote it: a cache that comes with a document (committed, or copied with it) can't put
//! its own shapes in the model. The entry's key is an HMAC under the secret too, so caches
//! of two secrets in one directory (one synced between two machines, say) keep their
//! entries side by side, never in each other's place. The directory ignores itself (a
//! `.gitignore` of `*`, put back if it's anything else), so it isn't committed with a
//! document in any repository; a directory that's a link, which may point anywhere, keeps
//! nothing.
//!
//! Only results are kept, never failures: a failure's cause may not last (memory, say), and
//! recompute keeps those that follow from the values in memory. Nothing here fails a
//! recompute: an entry that can't be read is a miss, and one that can't be written is left
//! unwritten, each with a warning. An entry that doesn't hold together (a bad seal, a
//! payload that doesn't parse, an entry for another key) is a miss too, rebuilt and written
//! over, never trusted. Without a secret (no home directory, say) the cache keeps nothing.
//!
//! The cache is bounded (docs/design/m2.5-cache-bound.md): a put that would take it past its
//! bound evicts the entries least recently used (written, or taken by a hit), and a scan
//! removes partial files left by writes that were killed. Eviction only makes misses: a miss
//! is a rebuild, which comes to the same bytes.

use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::kernel::{Blob, History, ShapeData, ShapeSummary};
use crate::recompute::{Built, Key};

/// The first bytes of every entry, naming its format: a change to the format changes it.
const MAGIC: &[u8] = b"silico cache 4\n";

/// What the cache's directory holds besides its entries: it ignores itself.
const GITIGNORE: &[u8] =
    b"# Silico's rebuild cache: rebuilt from the document, never committed.\n*\n";

/// The most bytes a cache keeps unless it's given another bound: twice the largest entry
/// ([`MAX_ENTRY`]), so even that is kept.
pub const DEFAULT_BOUND: u64 = 2 << 30;

/// How long after its last write a partial file is taken for a write that was killed, and
/// removed: a writer renames its partial within moments of writing its last byte.
pub const STALE_PARTIAL: Duration = Duration::from_secs(60 * 60);

/// A directory of kept results, for one platform, sealed with one secret.
#[derive(Clone)]
pub struct Cache {
    dir: PathBuf,
    platform: String,
    /// The secret entries are keyed and sealed with: none if it couldn't be had, and then
    /// nothing is kept.
    secret: Option<Secret>,
    /// Whether this cache (or a clone) has said its directory is a link it won't use.
    said_linked: Arc<AtomicBool>,
    /// The most bytes the directory's entries and partial files may take.
    bound: u64,
    /// What this cache and its clones know of the directory's size.
    usage: Arc<Mutex<Usage>>,
}

/// What a cache knows of its directory's size: the bytes its last scan found, plus those
/// it has written or is writing since (which `since_scan` counts too).
#[derive(Default)]
struct Usage {
    /// None until the first put scans.
    known: Option<u64>,
    since_scan: u64,
    /// The bytes of the entries being written: a scan may not see them yet (a job's
    /// dependents run, and put, while its entry is written).
    writing: u64,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("dir", &self.dir)
            .field("platform", &self.platform)
            .field("sealed", &self.secret.is_some())
            .field("bound", &self.bound)
            .finish()
    }
}

/// A user's secret: entries' keys and seals are HMAC-SHA-256 under it.
#[derive(Clone)]
struct Secret([u8; 32]);

/// An entry's own key: where it lives, and what it says it's for.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct EntryKey([u8; 32]);

/// An entry's seal: the HMAC of its payload.
struct Seal([u8; 32]);

impl Secret {
    fn mac(&self) -> Option<Hmac<Sha256>> {
        <Hmac<Sha256> as Mac>::new_from_slice(&self.0).ok()
    }

    /// The key of the entry for `parts` (the kernel's identity, the platform, and the step's
    /// key, in hex): the HMAC of the format's magic, then each part after its length (8
    /// bytes, little-endian), so no two different sets of parts key alike.
    fn entry_key(&self, parts: [&str; 3]) -> Option<EntryKey> {
        let mut mac = self.mac()?;
        mac.update(MAGIC);
        for part in parts {
            mac.update(&(part.len() as u64).to_le_bytes());
            mac.update(part.as_bytes());
        }
        Some(EntryKey(mac.finalize().into_bytes().into()))
    }

    fn seal(&self, payload: &[u8]) -> Option<Seal> {
        let mut mac = self.mac()?;
        mac.update(payload);
        Some(Seal(mac.finalize().into_bytes().into()))
    }

    /// Whether `seal` is `payload`'s, compared in constant time.
    fn sealed(&self, payload: &[u8], seal: &[u8]) -> bool {
        self.mac().is_some_and(|mut mac| {
            mac.update(payload);
            mac.verify_slice(seal).is_ok()
        })
    }
}

/// An entry's payload: what the seal covers.
#[derive(Serialize, Deserialize)]
struct Entry {
    /// The entry's own key, so an entry copied under another key's name isn't taken for it.
    key: EntryKey,
    brep: Vec<u8>,
    valid: bool,
    history: Option<History>,
    /// The shape's summary, if it was measured (a feature's solid is, once it's made).
    summary: Option<ShapeSummary>,
}

impl Cache {
    /// The cache in `dir` (`.silico-cache/` beside a document, say), for this platform,
    /// sealed with the user's secret: `cache-secret` in `$XDG_CACHE_HOME/silico`, or in
    /// `~/.cache/silico`, made the first time. The directory is made when the first entry
    /// is written.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let secret = match user_secret_file() {
            Some(file) => secret_in(&file),
            None => {
                tracing::warn!(
                    "no home directory for the cache's secret: keeping no results on disk"
                );
                None
            }
        };
        Self::new(dir.into(), this_platform(), secret)
    }

    /// The cache in `dir`, for this platform, sealed with the secret in `secret_file` (made
    /// if there's none): for tests and tools; a session uses [`open`](Self::open).
    pub fn with_secret(dir: impl Into<PathBuf>, secret_file: &Path) -> Self {
        Self::for_platform(dir, &this_platform(), secret_file)
    }

    /// The cache in `dir`, for `platform` (`macos-aarch64`, `linux-x86_64`), sealed with the
    /// secret in `secret_file` (made if there's none): for tests of other platforms.
    pub fn for_platform(dir: impl Into<PathBuf>, platform: &str, secret_file: &Path) -> Self {
        Self::new(dir.into(), platform.to_owned(), secret_in(secret_file))
    }

    fn new(dir: PathBuf, platform: String, secret: Option<Secret>) -> Self {
        Self {
            dir: plain(dir),
            platform,
            secret,
            said_linked: Arc::default(),
            bound: DEFAULT_BOUND,
            usage: Arc::default(),
        }
    }

    /// This cache, keeping at most `bound` bytes of entries and partial files (rather than
    /// [`DEFAULT_BOUND`]): an entry larger than that isn't kept.
    #[must_use]
    pub fn with_bound(mut self, bound: u64) -> Self {
        self.bound = bound;
        self
    }

    /// The result kept for `step` on `kernel` (its [`identity`](crate::kernel::Kernel::identity)),
    /// if there's a sound one.
    pub(crate) fn get(&self, kernel: &str, step: &Key) -> Option<Built> {
        let secret = self.secret()?;
        let key = secret.entry_key([kernel, &self.platform, &step.to_string()])?;
        let path = self.path(&key);
        if self.linked_below(&path) {
            return None;
        }
        let bytes = match read_file(&path) {
            FileRead::Bytes(bytes) => bytes,
            FileRead::Missing => return None,
            FileRead::Unusable(why) => {
                tracing::warn!(path = %path.display(), "a cache entry {why}: rebuilding it");
                let _ = std::fs::remove_file(&path);
                return None;
            }
            FileRead::Failed(e) => {
                tracing::warn!(path = %path.display(), "couldn't read a cache entry: {e}");
                return None;
            }
        };
        match read_entry(&bytes, secret, &key) {
            Some(entry) => {
                renew(&path);
                Some(Built::new(
                    ShapeData::checked(Blob::inline(entry.brep), entry.valid),
                    entry.history,
                    entry.summary,
                ))
            }
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
        let (Some(secret), Some(valid)) = (self.secret(), built.shape.valid()) else {
            return;
        };
        let Some(key) = secret.entry_key([kernel, &self.platform, &step.to_string()]) else {
            return;
        };
        let entry = Entry {
            key,
            brep: built.shape.brep().as_bytes().to_vec(),
            valid,
            history: built.history.clone(),
            summary: built.summary().cloned(),
        };
        let payload = match postcard::to_allocvec(&entry) {
            Ok(payload) => payload,
            Err(e) => {
                tracing::warn!("couldn't encode a cache entry: {e}");
                return;
            }
        };
        let Some(seal) = secret.seal(&payload) else {
            return;
        };
        let path = self.path(&key);
        if self.linked_below(&path) {
            return;
        }
        let size = (MAGIC.len() + seal.0.len() + payload.len()) as u64;
        if size > self.bound {
            tracing::debug!(
                size,
                bound = self.bound,
                "a cache entry larger than the cache's bound: not kept"
            );
            return;
        }
        if !self.make_room(size) {
            tracing::debug!(
                size,
                bound = self.bound,
                "no room for a cache entry beside those being written: not kept"
            );
            return;
        }
        let written = write_entry(&self.dir, &path, &seal, &payload);
        {
            let mut usage = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
            usage.writing = usage.writing.saturating_sub(size);
            if written.is_err() {
                usage.known = usage.known.map(|known| known.saturating_sub(size));
                usage.since_scan = usage.since_scan.saturating_sub(size);
            }
        }
        if let Err(e) = written {
            tracing::warn!(path = %path.display(), "couldn't write a cache entry: {e}");
        }
    }

    /// Makes room for an entry of `size` bytes, and counts it as being written. Scans the
    /// directory at the first put, once an eighth of the bound has been written since the
    /// last scan (so other sessions' writes are seen), and when the entry would take the
    /// cache past its bound; then evicts the entries least recently used, until what's left
    /// and the entry take at most three quarters of the bound. Returns false, counting
    /// nothing, if even then the entry would take the cache past its bound: the entries
    /// being written (by this cache's other puts at once) can't be evicted, so it's refused.
    fn make_room(&self, size: u64) -> bool {
        let mut usage = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
        let due = usage
            .known
            .is_none_or(|known| known.saturating_add(size) > self.bound)
            || usage.since_scan.saturating_add(size) > self.bound / 8;
        if due {
            let span = tracing::debug_span!(
                "cache.evict",
                dir = %self.dir.display(),
                entries = tracing::field::Empty,
                bytes = tracing::field::Empty,
                evicted = tracing::field::Empty,
                evicted_bytes = tracing::field::Empty,
                partials_removed = tracing::field::Empty,
            );
            let _entered = span.enter();
            let mut scan = scan(&self.dir, SystemTime::now());
            span.record("entries", scan.entries.len());
            span.record("bytes", scan.bytes);
            span.record("partials_removed", scan.partials_removed);
            // What's being written counts as kept, whether or not the scan saw it.
            let coming = u128::from(size) + u128::from(usage.writing);
            let bound = u128::from(self.bound);
            let target = (bound - bound / 4).saturating_sub(coming);
            let (evicted, evicted_bytes) = if scan.bytes + coming > bound {
                evict(&mut scan, target)
            } else {
                (0, 0)
            };
            span.record("evicted", evicted);
            span.record("evicted_bytes", evicted_bytes);
            let found = u64::try_from(scan.bytes).unwrap_or(u64::MAX);
            usage.known = Some(found.saturating_add(usage.writing));
            usage.since_scan = 0;
            if scan.bytes + coming > bound {
                return false;
            }
        }
        usage.known = usage.known.map(|known| known.saturating_add(size));
        usage.since_scan = usage.since_scan.saturating_add(size);
        usage.writing = usage.writing.saturating_add(size);
        true
    }

    /// The secret to key and seal entries with, if the cache keeps any: not when its
    /// directory is a link (one that came with a document may point anywhere: inside the
    /// repository, where its entries would be committed, or at anyone's files), which it
    /// says once. Nor is a subdirectory that's a link used, for the same reason.
    fn secret(&self) -> Option<&Secret> {
        let secret = self.secret.as_ref()?;
        if is_link(&self.dir) {
            self.say_linked(&self.dir);
            return None;
        }
        Some(secret)
    }

    /// Whether the subdirectory an entry at `path` lives in is a link, said once.
    fn linked_below(&self, path: &Path) -> bool {
        match path.parent().filter(|dir| is_link(dir)) {
            Some(dir) => {
                self.say_linked(dir);
                true
            }
            None => false,
        }
    }

    /// Says, once, that a directory of the cache is a link, which it doesn't use.
    fn say_linked(&self, dir: &Path) {
        if !self.said_linked.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                dir = %dir.display(),
                "a directory of the cache is a link: keeping no results through it"
            );
        }
    }

    /// Where an entry lives: under the first two hex digits of its key, so no directory
    /// holds every entry.
    fn path(&self, key: &EntryKey) -> PathBuf {
        let hex = crate::hex(&key.0);
        self.dir.join(&hex[..2]).join(&hex[2..])
    }
}

/// Whether `path` is a symbolic link (not followed).
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// `path` with no trailing slash (through which a link would be followed, not seen) and no
/// `.` components.
fn plain(path: PathBuf) -> PathBuf {
    path.components().collect()
}

/// What a scan of a cache's directory found.
struct Scan {
    /// Every entry, as its last use (its modification time), its path, and its size.
    entries: Vec<(SystemTime, PathBuf, u64)>,
    /// The bytes of its entries and of the partial files it left, by their lengths: wider
    /// than they are, since lengths (of sparse files) can sum past what a `u64` holds.
    bytes: u128,
    /// How many partial files, last written [`STALE_PARTIAL`] ago or more, it removed.
    partials_removed: usize,
}

/// Lists the entries of the cache in `dir`, and removes its partial files last written
/// [`STALE_PARTIAL`] before `now` or earlier. Reads metadata only, and follows no link: an
/// entry is a regular file named by 62 hex digits in a subdirectory named by two; a partial
/// is a file named `.partial-…` in either. Anything else is left, and not counted.
fn scan(dir: &Path, now: SystemTime) -> Scan {
    let mut scan = Scan {
        entries: Vec::new(),
        bytes: 0,
        partials_removed: 0,
    };
    let mut warned = false;
    for item in list(dir) {
        let name = item.file_name();
        let path = item.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if is_partial(&name) {
            scan.partial(&path, &meta, now, &mut warned);
        } else if meta.is_dir() && is_hex(&name, 2) {
            for file in list(&path) {
                let name = file.file_name();
                let path = file.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if is_partial(&name) {
                    scan.partial(&path, &meta, now, &mut warned);
                } else if meta.is_file() && is_hex(&name, 62) {
                    let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    scan.bytes = scan.bytes.saturating_add(u128::from(meta.len()));
                    scan.entries.push((used, path, meta.len()));
                }
            }
        }
    }
    scan
}

impl Scan {
    /// Removes the partial file at `path` if it was last written [`STALE_PARTIAL`] before
    /// `now` or earlier (a link that's named as one, by its own time: never what it points
    /// at); otherwise counts it.
    fn partial(
        &mut self,
        path: &Path,
        meta: &std::fs::Metadata,
        now: SystemTime,
        warned: &mut bool,
    ) {
        let written = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let stale = now
            .duration_since(written)
            .is_ok_and(|age| age >= STALE_PARTIAL);
        if stale && !meta.is_dir() {
            match remove(path, warned) {
                Removed::Yes | Removed::Gone => self.partials_removed += 1,
                Removed::No => {
                    if meta.is_file() {
                        self.bytes = self.bytes.saturating_add(u128::from(meta.len()));
                    }
                }
            }
        } else if meta.is_file() {
            self.bytes = self.bytes.saturating_add(u128::from(meta.len()));
        }
    }
}

/// Evicts `scan`'s entries, least recently used first (by path, of two used at once), until
/// its bytes are at most `target` or no entry is left. Returns how many it evicted, and
/// their bytes.
fn evict(scan: &mut Scan, target: u128) -> (usize, u128) {
    scan.entries.sort();
    let mut warned = false;
    let (mut evicted, mut evicted_bytes) = (0, 0_u128);
    for (_, path, size) in &scan.entries {
        if scan.bytes <= target {
            break;
        }
        match remove(path, &mut warned) {
            // Gone already: another session evicted it, or rebuilt it after a bad read.
            Removed::Yes | Removed::Gone => {
                scan.bytes = scan.bytes.saturating_sub(u128::from(*size));
                evicted += 1;
                evicted_bytes = evicted_bytes.saturating_add(u128::from(*size));
            }
            Removed::No => {}
        }
    }
    (evicted, evicted_bytes)
}

/// What removing a cache's file came to.
enum Removed {
    Yes,
    Gone,
    No,
}

/// Removes the file (or link) at `path`, warning of a failure (once for each `warned`).
fn remove(path: &Path, warned: &mut bool) -> Removed {
    match std::fs::remove_file(path) {
        Ok(()) => Removed::Yes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Removed::Gone,
        Err(e) => {
            if !std::mem::replace(warned, true) {
                tracing::warn!(path = %path.display(), "couldn't remove a cache file: {e}");
            }
            Removed::No
        }
    }
}

/// The items in the directory `dir`, or none if it can't be read.
fn list(dir: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
    std::fs::read_dir(dir).into_iter().flatten().flatten()
}

/// Whether `name` is a partial file's: one [`new_file`] makes.
fn is_partial(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".partial-")
}

/// Whether `name` is `digits` lowercase hex digits, as an entry's path is.
fn is_hex(name: &std::ffi::OsStr, digits: usize) -> bool {
    let name = name.as_encoded_bytes();
    name.len() == digits && name.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Marks the entry at `path` as used now, so it's evicted after those used before it. A
/// hit whose time can't be set (a read-only cache, say) is still a hit.
fn renew(path: &Path) {
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    if let Ok(file) = opened {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// The user's secret file: `silico/cache-secret` in `$XDG_CACHE_HOME`, if that's an absolute
/// path, or in `~/.cache`.
fn user_secret_file() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .map(|home| home.join(".cache"))
        })?;
    Some(cache.join("silico").join("cache-secret"))
}

/// The secret in `file`, made if there's none, or none (with a warning) if it can't be had.
fn secret_in(file: &Path) -> Option<Secret> {
    match read_or_make_secret(file) {
        Ok(secret) => Some(Secret(secret)),
        Err(e) => {
            tracing::warn!(
                file = %file.display(),
                "couldn't read or make the cache's secret ({e}): keeping no results on disk; \
                 if the file is damaged, remove it"
            );
            None
        }
    }
}

/// The 32 bytes in `file`; if there's no file, 32 new random bytes, written to it first.
fn read_or_make_secret(file: &Path) -> std::io::Result<[u8; 32]> {
    // Twice at most: a session that loses a race to make the file reads the winner's.
    for _ in 0..2 {
        match read_file(file) {
            FileRead::Bytes(bytes) => {
                return <[u8; 32]>::try_from(bytes.as_slice())
                    .map_err(|_| std::io::Error::other("it isn't 32 bytes"));
            }
            FileRead::Missing => {}
            FileRead::Unusable(why) => return Err(std::io::Error::other(format!("it {why}"))),
            FileRead::Failed(e) => return Err(e),
        }
        let mut secret = [0; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut secret)?;
        let dir = file
            .parent()
            .ok_or_else(|| std::io::Error::other("it has no directory"))?;
        std::fs::create_dir_all(dir)?;
        let (partial, mut out) = new_file(dir, 0o600)?;
        // Durable before it's linked: a secret lost after entries were sealed with it
        // would leave a damaged file in its place.
        let written = out.write_all(&secret).and_then(|()| out.sync_all());
        // Linked, not renamed, to its name: two sessions making it at once agree on one.
        let linked = written.and_then(|()| std::fs::hard_link(&partial, file));
        let _ = std::fs::remove_file(&partial);
        match linked {
            Ok(()) => return Ok(secret),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("it keeps changing"))
}

/// The most bytes an entry may have: a shape as large as a worker may send, twice over for
/// its history. Anything larger isn't an entry this cache wrote.
const MAX_ENTRY: u64 = 2 * silico_protocol::MAX_FRAME_LEN as u64;

/// What reading a cache file came to.
enum FileRead {
    Bytes(Vec<u8>),
    Missing,
    /// Not a file this cache could have written: a symbolic link, a pipe, a device, a
    /// directory, or too large. Read no further (a pipe would never end).
    Unusable(&'static str),
    Failed(std::io::Error),
}

/// The bytes of the regular file at `path`, read without following a symbolic link or
/// waiting on a pipe, and no more than [`MAX_ENTRY`] of them.
fn read_file(path: &Path) -> FileRead {
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let file = match opened {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FileRead::Missing,
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return FileRead::Unusable("is a symbolic link");
        }
        Err(e) => return FileRead::Failed(e),
    };
    match file.metadata() {
        Ok(meta) if !meta.is_file() => return FileRead::Unusable("isn't a file"),
        Ok(meta) if meta.len() > MAX_ENTRY => return FileRead::Unusable("is too large"),
        Ok(_) => {}
        Err(e) => return FileRead::Failed(e),
    }
    let mut bytes = Vec::new();
    match file.take(MAX_ENTRY + 1).read_to_end(&mut bytes) {
        Ok(_) if bytes.len() as u64 > MAX_ENTRY => FileRead::Unusable("is too large"),
        Ok(_) => FileRead::Bytes(bytes),
        Err(e) => FileRead::Failed(e),
    }
}

/// The platform's name: its OS and architecture (`macos-aarch64`).
fn this_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The entry `bytes` hold for `key`, if they hold together: the format's magic, a seal
/// under `secret` that matches the payload, a payload that parses, and the key it was
/// written for.
fn read_entry(bytes: &[u8], secret: &Secret, key: &EntryKey) -> Option<Entry> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (seal, payload) = rest.split_at_checked(32)?;
    if !secret.sealed(payload, seal) {
        return None;
    }
    let entry: Entry = postcard::from_bytes(payload).ok()?;
    (entry.key == *key).then_some(entry)
}

/// Writes an entry at `path`, in the cache at `root`, whole or not at all: to a file of its
/// own beside it, then renamed over it, so a reader never sees a part of one, and two
/// writers of the same entry (two sessions) leave one or the other. Not synced to the disk:
/// an entry a crash cuts short or empties doesn't hold together, so it's a miss. Its
/// modification time, its last use, is set to now, to the nanosecond (Linux's own moves only
/// once a tick).
fn write_entry(root: &Path, path: &Path, seal: &Seal, payload: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no directory"))?;
    std::fs::create_dir_all(root)?;
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    if is_link(dir) {
        return Err(std::io::Error::other("its directory is a link"));
    }
    ignore_itself(root);
    let (partial, mut file) = new_file(dir, 0o644)?;
    let written = file
        .write_all(MAGIC)
        .and_then(|()| file.write_all(&seal.0))
        .and_then(|()| file.write_all(payload))
        .and_then(|()| file.set_modified(SystemTime::now()));
    let renamed = written.and_then(|()| std::fs::rename(&partial, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    renamed
}

/// Gives the cache at `root` its `.gitignore`, unless it has it: written whole, over
/// anything else at its name (a file emptied by a failed write, or one that came with a
/// clone), as an entry is.
fn ignore_itself(root: &Path) {
    let path = root.join(".gitignore");
    if let FileRead::Bytes(bytes) = read_file(&path)
        && bytes == GITIGNORE
    {
        return;
    }
    let written = new_file(root, 0o644).and_then(|(partial, mut file)| {
        let renamed = file
            .write_all(GITIGNORE)
            .and_then(|()| std::fs::rename(&partial, &path));
        if renamed.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        renamed
    });
    if let Err(e) = written {
        tracing::warn!(path = %path.display(), "couldn't write the cache's .gitignore: {e}");
    }
}

/// A new file in `dir`, with permissions `mode`, never one already there (a link planted at
/// the name would otherwise be followed, and what it points at written over).
fn new_file(dir: &Path, mode: u32) -> std::io::Result<(PathBuf, std::fs::File)> {
    static FILES: AtomicU64 = AtomicU64::new(0);
    let mut attempt = 0;
    loop {
        let partial = dir.join(format!(
            ".partial-{}-{}",
            std::process::id(),
            FILES.fetch_add(1, Ordering::Relaxed)
        ));
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&partial);
        match created {
            Ok(file) => return Ok((partial, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 8 => {
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own for one test, removed after.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("silico-cache-unit-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(
                &self.0,
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
            );
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The cache's tests that make partial files, one at a time: [`new_file`]'s counter is
    /// the process's, and a test that plants names after it needs it to itself.
    static PARTIAL_FILES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_put_beside_entries_being_written_that_would_pass_the_bound_is_refused() {
        // Found by break-it: a put that a scan can't make room for (the entries being
        // written can't be evicted) was written anyway, so a session's puts at once passed
        // the bound. An empty directory, a bound of 100, and a put of 60 being written.
        let scratch = Scratch::new("refused");
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(100);
        *cache.usage.lock().unwrap() = Usage {
            known: Some(60),
            since_scan: 60,
            writing: 60,
        };
        // 60 + 50 is past 100: refused, and nothing counted for it.
        assert!(!cache.make_room(50));
        let usage = cache.usage.lock().unwrap();
        assert_eq!((usage.known, usage.writing), (Some(60), 60));
        drop(usage);
        // 60 + 40 is exactly 100: kept, and counted.
        assert!(cache.make_room(40));
        let usage = cache.usage.lock().unwrap();
        assert_eq!((usage.known, usage.writing), (Some(100), 100));
    }

    #[test]
    fn eviction_counts_entries_whose_lengths_sum_past_u64_max_exactly() {
        // Found by break-it: sparse entries' lengths can sum past what a u64 holds (a
        // plain add panicked; a saturating one undercounts, and leaves a planted entry).
        // Three entries of 2^63 bytes (gone already, so each is evicted as if by another
        // session), 3 * 2^63 in all, evicted to 2^63: the oldest two go, 2^64 bytes.
        let half = 1_u64 << 63;
        let gone = |n: u8| PathBuf::from(format!("/nonexistent-silico-cache/{n}"));
        let mut scan = Scan {
            entries: (0..3)
                .map(|n| (SystemTime::UNIX_EPOCH, gone(n), half))
                .collect(),
            bytes: 3 * u128::from(half),
            partials_removed: 0,
        };
        assert_eq!(evict(&mut scan, u128::from(half)), (2, 1_u128 << 64));
        assert_eq!(scan.bytes, u128::from(half));
        assert_eq!(scan.entries[2].1, gone(2));
    }

    #[test]
    fn a_cache_through_a_link_says_so_once_for_itself_and_its_clones() {
        // Found by the milestone's mutation run: saying it never, or every time but the
        // first, passed every test.
        use std::sync::Mutex;
        use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
        struct Warnings(Arc<Mutex<u32>>);
        impl<S: tracing::Subscriber> Layer<S> for Warnings {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                if *event.metadata().level() == tracing::Level::WARN {
                    *self.0.lock().unwrap() += 1;
                }
            }
        }
        let scratch = Scratch::new("said-once");
        let real = scratch.0.join("real");
        std::fs::create_dir(&real).unwrap();
        let linked = scratch.0.join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let cache = Cache::new(linked, this_platform(), Some(Secret([7; 32])));
        let warnings = Arc::new(Mutex::new(0));
        let subscriber = tracing_subscriber::registry().with(Warnings(Arc::clone(&warnings)));
        tracing::subscriber::with_default(subscriber, || {
            assert!(cache.secret().is_none());
            assert!(cache.secret().is_none());
            assert!(cache.clone().secret().is_none());
        });
        assert_eq!(*warnings.lock().unwrap(), 1);
    }

    #[test]
    fn a_secret_the_system_wont_link_into_place_fails_with_the_systems_reason() {
        // Found by the milestone's mutation run: only a name already taken (a session that
        // made the secret first) is tried again; any other failure to link it is the
        // system's, said as it is, not "it keeps changing". A name ending in a slash is
        // missing to read, and can't be linked to (ENOENT on Linux and macOS).
        let _partial_files = PARTIAL_FILES.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::new("secret-unlinkable");
        let mut name = scratch.0.join("cache-secret").into_os_string();
        name.push("/");
        let error = read_or_make_secret(Path::new(&name)).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT), "{error}");
        // Nothing left beside it.
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
    }

    #[test]
    fn a_cache_file_is_read_exactly_when_its_a_regular_file() {
        // Found by the milestone's mutation run: each of these came to a miss through
        // `Cache::get` whatever `read_file` said, so a wrong answer passed every test.
        let scratch = Scratch::new("read-file");
        let file = scratch.0.join("entry");
        std::fs::write(&file, b"an entry").unwrap();
        assert!(matches!(read_file(&file), FileRead::Bytes(b) if b == b"an entry"));
        assert!(matches!(
            read_file(&scratch.0.join("none")),
            FileRead::Missing
        ));
        // A link isn't followed, even to a regular file.
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(matches!(
            read_file(&link),
            FileRead::Unusable("is a symbolic link")
        ));
        // A pipe (opened without waiting for a writer, so it reads as empty) and a
        // directory aren't files.
        let fifo = scratch.0.join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(
            read_file(&fifo),
            FileRead::Unusable("isn't a file")
        ));
        assert!(matches!(
            read_file(&scratch.0),
            FileRead::Unusable("isn't a file")
        ));
        // A path through a file (ENOTDIR) is neither missing nor a link: a failure.
        assert!(matches!(
            read_file(&file.join("below")),
            FileRead::Failed(e) if e.raw_os_error() == Some(libc::ENOTDIR)
        ));
    }

    #[test]
    fn an_entry_is_at_most_twice_the_largest_frame() {
        // Found by the milestone's mutation run: a limit of the frame's size plus two
        // passed every test. A worker's frame is at most 512 MiB, so an entry (a shape and
        // its history) is at most 1 GiB.
        assert_eq!(silico_protocol::MAX_FRAME_LEN, 512 << 20);
        assert_eq!(MAX_ENTRY, 1 << 30);
    }

    #[test]
    fn a_file_of_exactly_the_most_an_entry_may_be_is_read_whole_and_one_byte_more_isnt_promptly() {
        // Found by the milestone's mutation run: the limit's three edges (the stat's `>`,
        // the read's `MAX_ENTRY + 1`, the read's `>`) were never met, so each one off by one
        // passed every test. A sparse file of exactly 1 GiB is read whole (zeros, about
        // 0.15 s of CPU), and one byte more is refused by its size, unread. Each buffer is
        // dropped at once.
        let scratch = Scratch::new("most");
        let file = scratch.0.join("entry");
        let f = std::fs::File::create(&file).unwrap();
        f.set_len(MAX_ENTRY).unwrap();
        let start = cpu_time::ThreadTime::now();
        match read_file(&file) {
            FileRead::Bytes(bytes) => assert_eq!(bytes.len() as u64, MAX_ENTRY),
            FileRead::Missing => panic!("missing"),
            FileRead::Unusable(why) => panic!("unusable: {why}"),
            FileRead::Failed(e) => panic!("failed: {e}"),
        }
        f.set_len(MAX_ENTRY + 1).unwrap();
        assert!(matches!(
            read_file(&file),
            FileRead::Unusable("is too large")
        ));
        let spent = start.elapsed();
        assert!(
            spent < std::time::Duration::from_secs(10),
            "took {spent:?} of CPU"
        );
    }

    #[test]
    fn a_file_larger_than_any_entry_is_refused_without_reading_it_promptly() {
        // Found by the milestone's mutation run: a sparse file of 64 GiB was refused by the
        // read's own limit after reading 1 GiB of it, which passed every test: refused by
        // its size, it costs an open and a stat, far under 50 ms of CPU; read to the limit,
        // a gigabyte copied in.
        let scratch = Scratch::new("huge");
        let file = scratch.0.join("entry");
        std::fs::File::create(&file)
            .unwrap()
            .set_len(64 << 30)
            .unwrap();
        let start = cpu_time::ThreadTime::now();
        let read = read_file(&file);
        let spent = start.elapsed();
        assert!(matches!(read, FileRead::Unusable("is too large")));
        assert!(
            spent < std::time::Duration::from_millis(50),
            "took {spent:?} of CPU"
        );
    }

    #[test]
    fn an_entry_isnt_written_where_its_subdirectory_cant_be_made() {
        // Found by the milestone's mutation run: a failure to make the entry's subdirectory
        // (here, the cache's directory read-only) went on, and failed later for another
        // reason, which passed every test; the reason is the subdirectory's.
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = Scratch::new("read-only-root");
        std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::create_dir(scratch.0.join("probe")).is_ok() {
            // Running as root: permissions don't bite.
            return;
        }
        let error = write_entry(
            &scratch.0,
            &scratch.0.join("ab").join("cdef"),
            &Seal([0; 32]),
            b"payload",
        )
        .unwrap_err();
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied,
            "{error}"
        );
    }

    #[test]
    fn a_partial_file_takes_the_ninth_name_after_eight_taken_and_no_tenth() {
        // Found by the milestone's mutation run: none of the tries after a name already
        // taken (a planted one) was tested. A new file is tried at a name, then at up to
        // eight more, each the counter's next: with the next eight taken, the ninth is used;
        // with the next nine taken, it fails, as the name it last tried was taken.
        let _partial_files = PARTIAL_FILES.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::new("partial-names");
        let pid = std::process::id();
        let named = |n: u64| scratch.0.join(format!(".partial-{pid}-{n}"));
        let (first, _) = new_file(&scratch.0, 0o600).unwrap();
        let prefix = format!(".partial-{pid}-");
        let n: u64 = first
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix(&prefix)
            .unwrap()
            .parse()
            .unwrap();
        for taken in n + 1..=n + 8 {
            std::fs::write(named(taken), b"planted").unwrap();
        }
        let (ninth, _) = new_file(&scratch.0, 0o600).unwrap();
        assert_eq!(ninth, named(n + 9));
        for taken in n + 10..=n + 18 {
            std::fs::write(named(taken), b"planted").unwrap();
        }
        let error = new_file(&scratch.0, 0o600).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error}");
        assert!(!named(n + 19).exists());
        // Planted names are left as they were.
        for taken in (n + 1..=n + 8).chain(n + 10..=n + 18) {
            assert_eq!(std::fs::read(named(taken)).unwrap(), b"planted");
        }
    }
}
