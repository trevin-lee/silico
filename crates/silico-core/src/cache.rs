//! The rebuild cache (`.silico-cache/`, ARCHITECTURE.md): each kernel step's result kept on
//! disk, so a document opened again, or by another session, rebuilds without the kernel. An
//! entry is a step's shape (its B-rep and the worker's word on its validity), history, and
//! measure, under one seal, keyed by the step's recompute key, the kernel's identity, and the
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
//! Only results are kept, never a step's failure: a failure's cause may not last (memory,
//! say), and recompute keeps those that follow from the values in memory. A result's
//! measure the kernel refused is kept with it only if the refusal is a fact about the
//! solid's values (invalid input, or an invalid result), as the worker said it, as data:
//! the core that reads it back words it, and judges again whether it's kept. Nothing here
//! fails a recompute: an entry that can't be read is a miss, and one that can't be written
//! is left unwritten, each with a warning. An entry that doesn't hold together (a bad seal,
//! a payload that doesn't parse, an entry for another key) is a miss too, rebuilt and
//! written over, never trusted. Without a secret (no home directory, say) the cache keeps
//! nothing.
//!
//! The cache is bounded (docs/design/m2.5-cache-bound.md): a put that would take it past its
//! bound evicts the entries least recently used (written, or taken by a hit), and a scan
//! removes partial files left by writes that were killed. Eviction only makes misses: a miss
//! is a rebuild, which comes to the same bytes.

use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::kernel::{Blob, History, ShapeData, ShapeSummary};
use crate::recompute::{Built, Key};
use silico_protocol::Failure;

/// The first bytes of every entry, naming its format: a change to the format changes it.
/// It's part of every entry's key too, so an entry of another format is never looked for.
const MAGIC: &[u8] = b"silico cache 6\n";

/// What the cache's directory holds besides its entries: it ignores itself.
const GITIGNORE: &[u8] =
    b"# Silico's rebuild cache: rebuilt from the document, never committed.\n*\n";

/// The most bytes a cache keeps unless it's given another bound: twice the largest entry
/// ([`MAX_ENTRY`]), so even that is kept.
pub const DEFAULT_BOUND: u64 = 2 * MAX_ENTRY;

/// How long after its last write a partial file is taken for a write that was killed, and
/// removed: a writer renames its partial within moments of writing its last byte. One
/// dated this far in the future (a clock that was wrong, a server's ahead) is taken for one
/// too.
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
    /// What this cache and its clones know of the directory's size, and the condition its
    /// first scan's end is told by.
    usage: Arc<(Mutex<Usage>, Condvar)>,
    /// Whether this cache (or a clone) has said it's full of files it can't evict.
    said_full: Arc<AtomicBool>,
}

/// What a cache knows of its directory's size: the bytes its last scan found, plus those
/// it has written or is writing since (which `since_scan` counts too).
#[derive(Default)]
struct Usage {
    /// None until the first scan ends.
    known: Option<u64>,
    since_scan: u64,
    /// The bytes of the entries being written: a scan may not see them yet (a job's
    /// dependents run, and put, while its entry is written).
    writing: u64,
    /// Whether a put is scanning the directory (without the lock).
    scanning: bool,
    /// How many scans this cache and its clones have made.
    scans: u64,
}

/// Whether a put may write its entry.
#[derive(Debug, PartialEq, Eq)]
enum Room {
    Made,
    /// No room beside the entries this cache is writing, which can't be evicted yet.
    Busy,
    /// No room beside files it can't evict: other processes' partial files not yet stale,
    /// and files it couldn't remove.
    Stuck,
    /// No room, but what's being written changed during the scan (puts began, which it
    /// couldn't make room for, or writes it counted ended, whose entries it may now evict):
    /// another scan may make room. Never returned by [`Cache::make_room`], which scans again
    /// once and then says [`Room::Busy`].
    Again,
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
    /// The shape's measure: none yet, its summary (a feature's solid is measured once it's
    /// made), or the kernel's refusal to measure it, for its values.
    measure: Measure,
}

/// An entry's measure.
#[derive(Serialize, Deserialize)]
enum Measure {
    Unmeasured,
    Summary(ShapeSummary),
    /// The kernel worker's refusal, as data, and the operation it was for (`Summarize`):
    /// the core reading it back words it, so its words are always that core's (the entry's
    /// key holds the worker's identity, not the core's). Only a worker's refusal of a
    /// measure is kept so (`worker::failure_error_with_edge_names` keeps no other's).
    Refused {
        op: String,
        failure: Failure,
    },
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
            said_full: Arc::default(),
        }
    }

    /// This cache, keeping at most `bound` bytes of entries and partial files (rather than
    /// [`DEFAULT_BOUND`]): an entry larger than that isn't kept. The bound is exact for one
    /// session; each other session on the directory may take it past by what it writes
    /// before its next scan (docs/design/m2.5-cache-bound.md).
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
        match read_entry(&bytes, secret, &key).map(built_of) {
            Some(built) => {
                renew(&path);
                Some(built)
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
            measure: match (built.summary(), built.measure_refused()) {
                (Some(summary), _) => Measure::Summary(summary.clone()),
                (None, Some(refused)) if lasting(refused) => match refused.worker_failure() {
                    Some((op, failure)) => Measure::Refused {
                        op: op.to_owned(),
                        failure: failure.clone(),
                    },
                    // Not a worker's (a fake kernel's): nothing to word it again from.
                    None => Measure::Unmeasured,
                },
                (None, _) => Measure::Unmeasured,
            },
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
        match self.make_room(size) {
            Room::Made => {}
            Room::Stuck => {
                self.say_full();
                return;
            }
            Room::Busy | Room::Again => {
                tracing::debug!(
                    size,
                    bound = self.bound,
                    "no room for a cache entry beside those being written: not kept"
                );
                return;
            }
        }
        let written = write_entry(&self.dir, &path, &seal, &payload);
        {
            let mut usage = self.usage();
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
    /// and the entry take at most three quarters of the bound. The scan runs without the
    /// lock: other puts that fit by what's known go on meanwhile, and those that don't wait
    /// for it (as all do for the first, before which nothing is known). Refuses, counting
    /// nothing, if even then the entry would take the cache past its bound, beside what
    /// can't be evicted: entries being written, partial files not yet stale, and files it
    /// couldn't remove.
    fn make_room(&self, size: u64) -> Room {
        self.make_room_scanning(size, Cache::scan_and_evict)
    }

    /// [`make_room`](Cache::make_room), each scan by `scan` (a test's runs a put during it).
    fn make_room_scanning(
        &self,
        size: u64,
        mut scan: impl FnMut(&Cache, u64, u64) -> Scan,
    ) -> Room {
        let (_, scanned) = &*self.usage;
        let over = |known: u64| known.saturating_add(size) > self.bound;
        let mut usage = self.usage();
        // While another put scans, one that fits by what's known goes on; one that doesn't
        // (or before the first scan, when nothing is known) waits for the scan to end.
        while usage.scanning && usage.known.is_none_or(over) {
            usage = scanned.wait(usage).unwrap_or_else(PoisonError::into_inner);
        }
        let due =
            usage.known.is_none_or(over) || usage.since_scan.saturating_add(size) > self.bound / 8;
        if due && !usage.scanning {
            let mut rescan = true;
            loop {
                usage.scanning = true;
                usage.since_scan = 0;
                usage.scans += 1;
                let writing = usage.writing;
                drop(usage);
                // Should the scan unwind, the puts waiting on it are let go all the same.
                let mut unwinding = ScanEnd {
                    cache: self,
                    armed: true,
                };
                let left = scan(self, size, writing);
                unwinding.armed = false;
                usage = self.usage();
                usage.scanning = false;
                scanned.notify_all();
                match settle(&mut usage, left.bytes, writing, size, self.bound) {
                    Room::Made => break,
                    // What was written during the scan took its room: puts begun meanwhile,
                    // or writes it counted that have ended. So it scans again, once, from
                    // the lock it holds now (no other put begins a scan in between): those
                    // written by then are entries to evict, those still being written are
                    // counted as coming, and only what changes during that scan can leave it
                    // no room, when it's refused as busy.
                    Room::Again if rescan => {
                        tracing::debug!(
                            size,
                            "the puts during a scan took its room: scanning again"
                        );
                        rescan = false;
                    }
                    Room::Again => return Room::Busy,
                    room => return room,
                }
            }
        }
        usage.known = usage.known.map(|known| known.saturating_add(size));
        usage.since_scan = usage.since_scan.saturating_add(size);
        usage.writing = usage.writing.saturating_add(size);
        Room::Made
    }

    /// Says, once, that the cache is full of files it can't evict.
    fn say_full(&self) {
        if !self.said_full.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                dir = %self.dir.display(),
                bound = self.bound,
                "the cache is full of files it can't evict (partial files of writes under \
                 way, or files it couldn't remove): keeping no more results until they're gone"
            );
        }
    }

    /// Scans the directory, removing stale partial files, and, if what it holds, `writing`
    /// bytes being written, and an entry of `size` would pass the bound, evicts until those
    /// take at most three quarters of it. Returns what's left.
    fn scan_and_evict(&self, size: u64, writing: u64) -> Scan {
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
        let mut found = scan(&self.dir, SystemTime::now());
        span.record("entries", found.entries.len());
        span.record("bytes", found.bytes);
        span.record("partials_removed", found.partials_removed);
        let coming = u128::from(size) + u128::from(writing);
        let bound = u128::from(self.bound);
        if found.bytes + coming > bound {
            let (evicted, evicted_bytes) =
                evict(&mut found, (bound - bound / 4).saturating_sub(coming));
            span.record("evicted", evicted);
            span.record("evicted_bytes", evicted_bytes);
        }
        found
    }

    /// What this cache knows of its directory's size, locked.
    fn usage(&self) -> MutexGuard<'_, Usage> {
        self.usage.0.lock().unwrap_or_else(PoisonError::into_inner)
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

/// Lets the puts waiting on a scan go if the scan unwinds (a panicking log layer, say),
/// unless disarmed first.
struct ScanEnd<'a> {
    cache: &'a Cache,
    armed: bool,
}

impl Drop for ScanEnd<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.cache.usage().scanning = false;
            self.cache.usage.1.notify_all();
        }
    }
}

/// Settles what `usage` knows after a scan that left `left` bytes on disk, begun while
/// `writing` bytes were being written: those, and all begun since (which the scan may or may
/// not have seen), count as kept. Then whether an entry of `size` fits within `bound`: if
/// not, whether only the cache's own writes are in its way.
fn settle(usage: &mut Usage, left: u128, writing: u64, size: u64, bound: u64) -> Room {
    let known = left + u128::from(writing) + u128::from(usage.since_scan);
    usage.known = Some(u64::try_from(known).unwrap_or(u64::MAX));
    let (size, bound) = (u128::from(size), u128::from(bound));
    if known + size <= bound {
        Room::Made
    } else if left + size > bound {
        Room::Stuck
    } else if usage.since_scan > 0 || usage.writing < writing {
        Room::Again
    } else {
        Room::Busy
    }
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
    /// Whether it has warned of a file it couldn't remove (once a scan).
    warned: bool,
}

/// Lists the entries of the cache in `dir`, and removes its partial files last written
/// [`STALE_PARTIAL`] or more from `now`. Reads metadata only, and follows no link: an entry
/// is a regular file named by 62 hex digits in a subdirectory named by two; a partial is a
/// file named `.partial-…` in either. Anything else is left, and not counted; nor are this
/// process's own partials not yet stale, which its puts count as being written (a process
/// keeps one cache, and its clones, for a directory: two would each miss the other's).
fn scan(dir: &Path, now: SystemTime) -> Scan {
    let mut found = Scan {
        entries: Vec::new(),
        bytes: 0,
        partials_removed: 0,
        warned: false,
    };
    let own = format!(".partial-{}-", std::process::id());
    for item in list(dir) {
        let name = item.file_name();
        let path = item.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if is_partial(&name) {
            found.remove_or_count_partial(&path, &meta, now, &own);
        } else if meta.is_dir() && is_hex(&name, 2) {
            for file in list(&path) {
                let name = file.file_name();
                let path = file.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if is_partial(&name) {
                    found.remove_or_count_partial(&path, &meta, now, &own);
                } else if meta.is_file() && is_hex(&name, 62) {
                    let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    found.bytes = found.bytes.saturating_add(u128::from(meta.len()));
                    found.entries.push((used, path, meta.len()));
                }
            }
        }
    }
    found
}

impl Scan {
    /// Removes the partial file at `path` if it was last written [`STALE_PARTIAL`] or more
    /// from `now`, before or after (a link that's named as one, by its own time: never what
    /// it points at); otherwise counts it, unless it's this process's (named from `own`).
    fn remove_or_count_partial(
        &mut self,
        path: &Path,
        meta: &std::fs::Metadata,
        now: SystemTime,
        own: &str,
    ) {
        let written = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let apart = match now.duration_since(written) {
            Ok(age) => age,
            Err(ahead) => ahead.duration(),
        };
        if apart >= STALE_PARTIAL && !meta.is_dir() && self.remove(path) {
            self.partials_removed += 1;
        } else if meta.is_file()
            && !path
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(own.as_bytes()))
        {
            self.bytes = self.bytes.saturating_add(u128::from(meta.len()));
        }
    }

    /// Removes the file (or link) at `path`, warning of a failure (once a scan): whether
    /// it's gone, as it is if another session removed it first.
    fn remove(&mut self, path: &Path) -> bool {
        match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => {
                if !std::mem::replace(&mut self.warned, true) {
                    tracing::warn!(path = %path.display(), "couldn't remove a cache file: {e}");
                }
                false
            }
        }
    }
}

/// Evicts `found`'s entries, least recently used first (by path, of two used at once),
/// until its bytes are at most `target` or no entry is left. Returns how many it evicted,
/// and their bytes.
fn evict(found: &mut Scan, target: u128) -> (usize, u128) {
    let mut entries = std::mem::take(&mut found.entries);
    entries.sort();
    let (mut evicted, mut evicted_bytes) = (0, 0_u128);
    let mut kept = Vec::new();
    for (used, path, size) in entries {
        // Gone already counts as evicted: another session evicted it, or rebuilt it after a
        // bad read.
        if found.bytes > target && found.remove(&path) {
            found.bytes = found.bytes.saturating_sub(u128::from(size));
            evicted += 1;
            evicted_bytes = evicted_bytes.saturating_add(u128::from(size));
        } else {
            kept.push((used, path, size));
        }
    }
    found.entries = kept;
    (evicted, evicted_bytes)
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
    if let Ok(file) = open_to_read(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// The file at `path`, opened to read without following a symbolic link or waiting on a
/// pipe.
fn open_to_read(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
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
    let file = match open_to_read(path) {
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

/// Whether a measure's refusal is a fact about the solid's values, so it's kept on disk:
/// invalid input, or an invalid result. Within what recompute keeps for the session
/// (`recompute::kept`), and must stay so. Not `kernel_failed`, though recompute keeps it for
/// the session as it keeps a step's: the cache keeps nothing a restart might not see again
/// (M2.4's decision, of OCCT's catch-all alerts), and OCCT's integration catches its own
/// shortage of memory (`math_ComputeGaussPointsAndWeights` and its Kronrod twin catch every
/// `Standard_Failure`, `Standard_OutOfMemory` too), which can then come back as a failed
/// measure. (A B-rep refused as damaged is invalid input too: a fact while shapes travel
/// inline, as they do; a transport that could damage them would make it not one.)
fn lasting(refused: &crate::Error) -> bool {
    matches!(
        refused.code(),
        crate::ErrorCode::InvalidInput | crate::ErrorCode::InvalidResult
    )
}

/// The result `entry` keeps, checked as a worker's reply is: a summary no shape has is a
/// measure to make again, and a refusal is worded now, by this core, and kept only if the
/// cache would keep it (this core may judge it otherwise than the one that wrote it).
fn built_of(entry: Entry) -> Built {
    let shape = ShapeData::checked(Blob::inline(entry.brep), entry.valid);
    match entry.measure {
        Measure::Summary(summary) if crate::kernel::summary_fault(&summary).is_none() => {
            Built::new(shape, entry.history, Some(summary))
        }
        Measure::Refused { op, failure } => {
            let refused = crate::worker::failure_error(&op, failure);
            if lasting(&refused) {
                Built::refused_measure(shape, entry.history, refused)
            } else {
                Built::new(shape, entry.history, None)
            }
        }
        Measure::Unmeasured | Measure::Summary(_) => Built::new(shape, entry.history, None),
    }
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
        .and_then(|()| file.write_all(payload));
    // Where the file system won't take a time (vfat, some network shares), it keeps its
    // own: the entry is still kept.
    let _ = file.set_modified(SystemTime::now());
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
        *cache.usage() = Usage {
            known: Some(60),
            since_scan: 60,
            writing: 60,
            scanning: false,
            scans: 0,
        };
        // 60 + 50 is past 100: refused, for what's being written, and nothing counted.
        assert_eq!(cache.make_room(50), Room::Busy);
        let usage = cache.usage();
        assert_eq!((usage.known, usage.writing), (Some(60), 60));
        drop(usage);
        // 60 + 40 is exactly 100: kept, and counted.
        assert_eq!(cache.make_room(40), Room::Made);
        let usage = cache.usage();
        assert_eq!((usage.known, usage.writing), (Some(100), 100));
    }

    /// An entry's file in `dir`, named by `byte` repeated, of `len` bytes, last used `ago`.
    fn entry_file(dir: &Path, byte: char, len: usize, ago: Duration) -> PathBuf {
        let sub = dir.join(byte.to_string().repeat(2));
        std::fs::create_dir_all(&sub).unwrap();
        let path = sub.join(byte.to_string().repeat(62));
        std::fs::write(&path, vec![0; len]).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - ago).unwrap();
        path
    }

    #[test]
    fn a_put_whose_room_puts_during_its_scan_took_scans_again_and_is_kept() {
        // Found under load (cache_bound's sixty documents in one session, 10 of 40 runs): a
        // put whose scan the document's other puts overlapped was refused as busy, though
        // those were written by then and could be evicted, so the newest step wasn't kept.
        // A bound of 100; on disk an entry of 40 used 20 s ago and one of 20 used 10 s ago; a
        // put of 30, and two of 40 written whole during its first scan, 2 s ago and now.
        let scratch = Scratch::new("rescan");
        let a = entry_file(&scratch.0, 'a', 40, Duration::from_secs(20));
        let b = entry_file(&scratch.0, 'b', 20, Duration::from_secs(10));
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(100);
        let mut scans = 0;
        let (mut c, mut d) = (PathBuf::new(), PathBuf::new());
        let room = cache.make_room_scanning(30, |cache, size, writing| {
            let left = cache.scan_and_evict(size, writing);
            scans += 1;
            if scans == 1 {
                c = entry_file(&cache.dir, 'c', 40, Duration::from_secs(2));
                d = entry_file(&cache.dir, 'd', 40, Duration::ZERO);
                cache.usage().since_scan += 80;
            }
            left
        });
        // By hand: the first scan finds 60, and 60 + 30 is within 100, so it evicts nothing;
        // with the 80 put since, 60 + 80 + 30 is past 100, but those 80 are in place, so it
        // scans again: a, b, c, d are 140, past 100 with the 30, and it evicts the least
        // recently used, a, b, and c, to 40, within 75 - 30; 40 + 30 fits.
        assert_eq!(room, Room::Made);
        assert_eq!(scans, 2);
        assert!(!a.exists() && !b.exists() && !c.exists() && d.exists());
        let usage = cache.usage();
        assert_eq!(
            (usage.known, usage.since_scan, usage.writing),
            (Some(70), 30, 30)
        );
    }

    #[test]
    fn a_put_whose_scans_puts_keep_taking_its_room_scans_again_once_and_is_refused() {
        // As above, but 80 more written during every scan: it scans again once, then takes
        // what's still coming to be its own writes, and is refused (a miss later), rather
        // than scanning without end.
        let scratch = Scratch::new("rescan-busy");
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(100);
        let mut scans = 0;
        let room = cache.make_room_scanning(30, |cache, size, writing| {
            let left = cache.scan_and_evict(size, writing);
            scans += 1;
            assert!(scans <= 3, "it scanned again and again");
            let byte = if scans == 1 { 'e' } else { 'f' };
            entry_file(&cache.dir, byte, 80, Duration::ZERO);
            cache.usage().since_scan += 80;
            left
        });
        assert_eq!(room, Room::Busy);
        assert_eq!(scans, 2);
        // Nothing counted for it: what the second scan settled on (f, put during it).
        let usage = cache.usage();
        assert_eq!(
            (usage.known, usage.since_scan, usage.writing),
            (Some(80), 80, 0)
        );
    }

    #[test]
    fn a_put_begun_during_a_scan_and_still_being_written_is_counted_as_coming_by_the_next() {
        // Found by review: no test had a put begun during a scan still being written at its
        // end, which the next scan must count as coming, beside the put it makes room for (a
        // scan counting it nowhere would let the bound pass). A bound of 100, known to hold
        // a (30, used 20 s ago) and b (30, 10 s ago); a put of 30, and, during its first scan,
        // another session thread's put of 40 that fits by what's known, made room for and
        // still being written.
        let scratch = Scratch::new("rescan-writing");
        let a = entry_file(&scratch.0, 'a', 30, Duration::from_secs(20));
        let b = entry_file(&scratch.0, 'b', 30, Duration::from_secs(10));
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(100);
        cache.usage().known = Some(60);
        let mut scans = 0;
        let room = cache.make_room_scanning(30, |cache, size, writing| {
            let left = cache.scan_and_evict(size, writing);
            scans += 1;
            if scans == 1 {
                assert_eq!(cache.make_room(40), Room::Made);
            }
            left
        });
        // By hand: the first scan finds 60, and 60 + 30 is within 100: nothing evicted; with
        // the 40 begun since, 60 + 40 + 30 is past 100, so it scans again, now counting the
        // 40 as coming: 60 + 40 + 30 is past 100, so it evicts to 75 - 70 = 5, a and b
        // both; 0 left, 40 being written, and 30 fit (70).
        assert_eq!(room, Room::Made);
        assert_eq!(scans, 2);
        assert!(!a.exists() && !b.exists());
        let usage = cache.usage();
        assert_eq!(
            (usage.known, usage.since_scan, usage.writing),
            (Some(70), 30, 70)
        );
    }

    #[test]
    fn a_put_that_fills_the_cache_exactly_doesnt_scan_and_one_byte_more_does() {
        // 760 known of a bound of 800, nothing written since the last scan: a put of 40 makes
        // exactly 800, within the bound, so it neither scans nor waits for one; a put of one
        // byte more would pass it, and scans (an empty directory: room is made).
        let scratch = Scratch::new("exactly");
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(800);
        cache.usage().known = Some(760);
        assert_eq!(cache.make_room(40), Room::Made);
        let usage = cache.usage();
        assert_eq!((usage.known, usage.scans), (Some(800), 0));
        drop(usage);
        let other =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(800);
        other.usage().known = Some(760);
        assert_eq!(other.make_room(41), Room::Made);
        assert_eq!(other.usage().scans, 1);
    }

    #[test]
    fn a_put_kept_out_by_another_processs_partial_is_stuck_and_by_its_own_isnt() {
        // Found by review: a refusal for files the cache can't evict (here, another
        // process's partial, not yet stale, past the bound alone) is warned of, unlike one
        // for its own entries being written. This process's own partials aren't counted:
        // its puts count them as being written.
        let scratch = Scratch::new("stuck");
        let sub = scratch.0.join("ab");
        std::fs::create_dir_all(&sub).unwrap();
        let own = sub.join(format!(".partial-{}-0", std::process::id()));
        std::fs::write(&own, vec![0; 200]).unwrap();
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(100);
        assert_eq!(cache.make_room(10), Room::Made);
        assert_eq!(cache.usage().known, Some(10));
        let theirs = sub.join(".partial-1-0");
        std::fs::write(&theirs, vec![0; 200]).unwrap();
        let fresh =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(100);
        assert_eq!(fresh.make_room(10), Room::Stuck);
        assert!(own.exists() && theirs.exists());
    }

    #[test]
    fn a_scan_settles_on_what_it_left_what_was_being_written_and_all_begun_since() {
        // Found by review: what puts that went on during a scan begun was never counted in
        // any test, nor what's left beside the cache's own writes. A bound of 100 each time.
        let usage = |since_scan| Usage {
            since_scan,
            ..Usage::default()
        };
        // Nothing left, 70 being written as the scan began, 20 begun since: 90 kept, and a
        // put of 20 doesn't fit, though only for the cache's own writes, and those begun
        // since may be in place by now: scan again.
        let mut after = usage(20);
        assert_eq!(settle(&mut after, 0, 70, 20, 100), Room::Again);
        assert_eq!(after.known, Some(90));
        // 30 left, 40 being written, still: 70; a put of 40 doesn't fit, and 30 left beside
        // it would: busy, not stuck.
        let mut after = Usage {
            writing: 40,
            ..usage(0)
        };
        assert_eq!(settle(&mut after, 30, 40, 40, 100), Room::Busy);
        assert_eq!(after.known, Some(70));
        // The same, but those 40 written by the scan's end: in place, so another scan may
        // evict them.
        let mut after = usage(0);
        assert_eq!(settle(&mut after, 30, 40, 40, 100), Room::Again);
        // 90 left, that it couldn't evict: a put of 20 is stuck.
        let mut after = usage(0);
        assert_eq!(settle(&mut after, 90, 0, 20, 100), Room::Stuck);
        // 60 left and 10 being written, still: a put of 40 doesn't fit, but it would beside
        // what's left alone (exactly 100): busy.
        let mut after = Usage {
            writing: 10,
            ..usage(0)
        };
        assert_eq!(settle(&mut after, 60, 10, 40, 100), Room::Busy);
        // 90 left that it couldn't evict, and 20 begun since: stuck, whatever else is being
        // written, not scanned again.
        let mut after = usage(20);
        assert_eq!(settle(&mut after, 90, 0, 20, 100), Room::Stuck);
        // 10 left, 30 being written, 20 begun since: 60, and a put of 40 makes exactly 100.
        let mut after = usage(20);
        assert_eq!(settle(&mut after, 10, 30, 40, 100), Room::Made);
        assert_eq!(after.known, Some(60));
    }

    #[test]
    fn a_cache_scans_at_its_first_put_and_after_each_eighth_of_its_bound() {
        // Found by review: scanning at every put passed every test, costing only time (1.5 s
        // a put for a full cache). An empty directory; puts that fit, so no scan is for
        // room. Under the default bound, ten puts of 100 scan once, at the first.
        let scratch = Scratch::new("scans");
        let cache = Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32])));
        for _ in 0..10 {
            assert_eq!(cache.make_room(100), Room::Made);
        }
        assert_eq!(cache.usage().scans, 1);
        // A bound of 800, an eighth of 100: puts of 50 scan at the first (50 since), not
        // at the next (exactly 100), and when 50 more would make 150: the 3rd, 5th, 7th,
        // and 9th.
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(800);
        for _ in 0..9 {
            assert_eq!(cache.make_room(50), Room::Made);
        }
        assert_eq!(cache.usage().scans, 5);
        // A clone shares what its cache knows (50 since): no scan of its own.
        let twin = cache.clone();
        assert_eq!(twin.make_room(10), Room::Made);
        assert_eq!(twin.usage().scans, 5);
    }

    #[test]
    fn a_scan_that_unwinds_lets_the_puts_after_it_go() {
        // Found by review: nothing made a scan unwind, so letting its waiters go was never
        // tested. A log layer that panics as the scan opens its span: the put unwinds, the
        // scan is over, and the next put (with the layer gone) scans and is let in, not
        // left waiting on a scan that never ends.
        use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
        struct PanicAtScan;
        impl<S: tracing::Subscriber> Layer<S> for PanicAtScan {
            fn on_new_span(
                &self,
                attrs: &tracing::span::Attributes<'_>,
                _: &tracing::span::Id,
                _: Context<'_, S>,
            ) {
                if attrs.metadata().name() == "cache.evict" {
                    panic!("a log layer panics, as a test asks");
                }
            }
        }
        let scratch = Scratch::new("unwinds");
        let cache = Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32])));
        let subscriber = tracing_subscriber::registry().with(PanicAtScan);
        let unwound = tracing::subscriber::with_default(subscriber, || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cache.make_room(10)))
        });
        assert!(unwound.is_err());
        assert!(!cache.usage().scanning);
        assert_eq!(cache.make_room(10), Room::Made);
        assert_eq!(cache.usage().scans, 2);
    }

    #[test]
    fn a_full_cache_says_so_once_for_itself_and_its_clones() {
        // Found by review: saying it at every refusal but the first passed every test.
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
        let scratch = Scratch::new("full");
        let cache = Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32])));
        let warnings = Arc::new(Mutex::new(0));
        let subscriber = tracing_subscriber::registry().with(Warnings(Arc::clone(&warnings)));
        tracing::subscriber::with_default(subscriber, || {
            cache.say_full();
            cache.say_full();
            cache.clone().say_full();
        });
        assert_eq!(*warnings.lock().unwrap(), 1);
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
            warned: false,
        };
        assert_eq!(evict(&mut scan, u128::from(half)), (2, 1_u128 << 64));
        assert_eq!(scan.bytes, u128::from(half));
        // What's left is the newest.
        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].1, gone(2));
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
