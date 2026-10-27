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
//! nothing. So does one that leads out of its sessions' workspace (decision 26 of
//! docs/design/m4.1-command-api.md; [`Cache::within`]).
//!
//! Each read, write (with the eviction it makes room by), and clear opens the directory
//! once, from a handle on its document's directory, never through a link at its own name,
//! and does everything it does through that handle: every file below it is found from the
//! handle a name at a time (each subdirectory opened from it, never through a link, and
//! walked by its own handle), never by a path. So nothing done to the paths on its way once
//! the handle is open (a link put at its name, at its document's directory, or above) takes
//! the operation anywhere else; in a workspace, one done before is refused, as the system is
//! asked where the document's directory's handle is, and it must be in the workspace
//! ([`Cache::within`]). The handle is what keeps other directories safe, not the names the
//! cache acts on alone: its layout of entries (two hex digits, then 62) is a SHA-256 git
//! repository's of loose objects.
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

use std::ffi::{CStr, CString, OsStr};
use std::io::{Read as _, Write as _};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use hmac::{Hmac, Mac};
use rustix::fs::{CWD, Mode};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::file::{self, Unresolved, Workspace};
use crate::kernel::{Blob, History, ShapeData, ShapeSummary};
use crate::recompute::{Built, Described, Key};
use silico_protocol::{Description, Failure};

mod at;
use at::Meta;

/// The first bytes of every entry, naming its format: a change to the format changes it.
/// It's part of every entry's key too, so an entry of another format is never looked for.
const MAGIC: &[u8] = b"silico cache 8\n";

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
    /// The workspace its directory must lead nowhere out of ([`Cache::within`]), or none (the
    /// CLI's).
    workspace: Option<Workspace>,
    /// The workspace's root as the system names an open handle of it ([`at::path_of`]),
    /// taken once by [`Cache::within`]: a handle's path is compared with this spelling, the
    /// system's own, as well as the root's canonical one, since the two can differ (macOS
    /// names `/System/Volumes/Data/Users/...` as `/Users/...` through its firmlink).
    workspace_as_held: Option<PathBuf>,
    /// Whether this cache (or a clone) has said its directory is a link it won't use.
    said_linked: Arc<AtomicBool>,
    /// Whether this cache (or a clone) has said its directory leads out of its workspace.
    said_outside: Arc<AtomicBool>,
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
            .field("workspace", &self.workspace.as_ref().map(Workspace::root))
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
    /// The shape's measure: none yet, its summary and what's known of its description (a
    /// feature's solid is measured once it's made), or the kernel's refusal to measure it,
    /// for its values.
    measure: Measure,
}

/// An entry's measure.
#[derive(Serialize, Deserialize)]
enum Measure {
    Unmeasured,
    Measured {
        summary: ShapeSummary,
        description: KeptDescription,
    },
    /// The kernel worker's refusal of its summary, as data (as [`KeptDescription::Refused`]).
    Refused {
        op: String,
        failure: Failure,
    },
}

/// What an entry keeps of its shape's description: nothing (not asked, or a worker failed,
/// so it's asked again), the description, or the kernel worker's refusal of it, as data,
/// and the operation it was for (`Describe`; `worker::MEASURES`): the core reading it back
/// words it, so its words are always that core's (the entry's key holds the worker's
/// identity, not the core's). Only a worker's refusal of a measure is kept so
/// (`worker::failure_error_with_edge_names` keeps no other's).
#[derive(Serialize, Deserialize)]
enum KeptDescription {
    Unknown,
    Described(Description),
    Refused { op: String, failure: Failure },
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

    /// A cache in `dir` that keeps nothing and reads nothing, never reading or making the
    /// user's secret: for a client that rebuilds nothing (`silico show`), whose sessions
    /// need a cache to open.
    pub fn unsealed(dir: impl Into<PathBuf>) -> Self {
        Self::new(dir.into(), this_platform(), None)
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
            workspace: None,
            workspace_as_held: None,
            said_linked: Arc::default(),
            said_outside: Arc::default(),
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

    /// This cache, in `workspace`: the workspace of the `Sessions` it serves
    /// (`Sessions::in_workspace`; decision 26 of docs/design/m4.1-command-api.md). It keeps
    /// results only in a directory in the workspace, checked as each read, write (with the
    /// eviction it makes room by), and clear opens the directory, since a link may be put on
    /// its way at any time (its own name, its document's directory, or one above it, in a
    /// transaction that reads nothing again). Each opens its document's directory, through
    /// any link on the way (a document may be reached through a link inside the
    /// workspace), and asks the system where that handle is (macOS's `F_GETPATH`, Linux's
    /// `/proc/self/fd`): it must be in the workspace, or it's not used. Never a path
    /// resolved again, which a link flipped on the way between the open and the check would
    /// take elsewhere than the handle. Then `.silico-cache` is opened from that handle, never
    /// through a link (and made from it first, for a write), and nothing below is found by a
    /// path. So a link put on the way before the handle opens is refused, and one put after
    /// redirects nothing. A cache whose directory leads out is one that keeps nothing, as
    /// one whose directory is a link is (M2.5): a read misses, and a write, an eviction, and
    /// a clear do nothing, said once while it leads out (and again if it leads out again
    /// after it came back). What the handle can't help: its document's directory (or the
    /// cache's own), moved out of the workspace whole while an operation has it open, takes
    /// that operation's work with it, its own files only, since they're what the handle
    /// holds. A cache never changes a result, so a rebuild through a cache that keeps
    /// nothing is still right, only uncached. Without a workspace, the same opens are made,
    /// and the directory is used wherever the path to it leads.
    #[must_use]
    pub fn within(mut self, workspace: Workspace) -> Self {
        self.workspace_as_held = at::open_dir_through_links(workspace.root())
            .and_then(at::path_of)
            .ok();
        self.workspace = Some(workspace);
        self
    }

    /// The result kept for `step` on `kernel` (its [`identity`](crate::kernel::Kernel::identity)),
    /// if there's a sound one.
    pub(crate) fn get(&self, kernel: &str, step: &Key) -> Option<Built> {
        let secret = self.secret.as_ref()?;
        let key = secret.entry_key([kernel, &self.platform, &step.to_string()])?;
        let root = self.opened(false)?;
        let (subdirectory, name) = entry_names(&key);
        let path = self.path(&key);
        let dir = match at::open_dir(&root, subdirectory.as_str()) {
            Ok(dir) => dir,
            Err(Errno::NOENT) => return None,
            Err(_) if self.linked_below(&root, &subdirectory) => return None,
            Err(e) => {
                tracing::warn!(path = %path.display(), "couldn't read a cache entry: {e}");
                return None;
            }
        };
        let bytes = match read_file(&dir, name.as_str()) {
            FileRead::Bytes(bytes) => bytes,
            FileRead::Missing => return None,
            FileRead::Unusable(why) => {
                tracing::warn!(path = %path.display(), "a cache entry {why}: rebuilding it");
                let _ = at::remove(&dir, name.as_str());
                return None;
            }
            FileRead::Failed(e) => {
                tracing::warn!(path = %path.display(), "couldn't read a cache entry: {e}");
                return None;
            }
        };
        match read_entry(&bytes, secret, &key).map(built_of) {
            Some(built) => {
                renew(&dir, &name);
                Some(built)
            }
            None => {
                tracing::warn!(path = %path.display(), "a cache entry doesn't hold together: rebuilding it");
                let _ = at::remove(&dir, name.as_str());
                None
            }
        }
    }

    /// Keeps `built`, the result of `step` on `kernel`, if the worker said whether its shape
    /// is valid (a shape it hasn't checked isn't kept, so a hit is always checked).
    pub(crate) fn put(&self, kernel: &str, step: &Key, built: &Built) {
        let (Some(secret), Some(valid)) = (self.secret.as_ref(), built.shape.valid()) else {
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
                (Some(summary), _) => Measure::Measured {
                    summary: summary.clone(),
                    description: match built.described() {
                        Some(Described::Whole(description)) => {
                            KeptDescription::Described(description.clone())
                        }
                        Some(Described::Refused(refused)) if lasting(refused) => {
                            match refused.worker_failure() {
                                Some((op, failure)) => KeptDescription::Refused {
                                    op: op.to_owned(),
                                    failure: failure.clone(),
                                },
                                // Not a worker's (a fake kernel's): nothing to word it again from.
                                None => KeptDescription::Unknown,
                            }
                        }
                        Some(Described::Refused(_) | Described::Failed(_)) | None => {
                            KeptDescription::Unknown
                        }
                    },
                },
                (None, Some(refused)) if lasting(refused) => match refused.worker_failure() {
                    Some((op, failure)) => Measure::Refused {
                        op: op.to_owned(),
                        failure: failure.clone(),
                    },
                    // Not a worker's (a fake kernel's): nothing to word it again from.
                    None => Measure::Unmeasured,
                },
                _ => Measure::Unmeasured,
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
        let size = (MAGIC.len() + seal.0.len() + payload.len()) as u64;
        if size > self.bound {
            tracing::debug!(
                size,
                bound = self.bound,
                "a cache entry larger than the cache's bound: not kept"
            );
            return;
        }
        let Some(root) = self.opened(true) else {
            return;
        };
        let (subdirectory, name) = entry_names(&key);
        if self.linked_below(&root, &subdirectory) {
            return;
        }
        match self.make_room(&root, size) {
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
        let written = write_entry(&root, &self.dir, &subdirectory, &name, &seal, &payload);
        {
            let mut usage = self.usage();
            usage.writing = usage.writing.saturating_sub(size);
            if written.is_err() {
                usage.known = usage.known.map(|known| known.saturating_sub(size));
                usage.since_scan = usage.since_scan.saturating_sub(size);
            }
        }
        if let Err(e) = written {
            let path = self.path(&key);
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
    fn make_room(&self, root: &OwnedFd, size: u64) -> Room {
        self.make_room_scanning(root, size, Cache::scan_and_evict)
    }

    /// [`make_room`](Cache::make_room) in the directory `root` holds open, each scan by
    /// `scan` (a test's runs a put during it).
    fn make_room_scanning(
        &self,
        root: &OwnedFd,
        size: u64,
        mut scan: impl FnMut(&Cache, &OwnedFd, u64, u64) -> Scan,
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
                let left = scan(self, root, size, writing);
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

    /// Scans the directory `root` holds open, removing stale partial files, and, if what it
    /// holds, `writing` bytes being written, and an entry of `size` would pass the bound,
    /// evicts until those take at most three quarters of it. Returns what's left.
    fn scan_and_evict(&self, root: &OwnedFd, size: u64, writing: u64) -> Scan {
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
        let mut found = scan(root, &self.dir, SystemTime::now());
        span.record("entries", found.entries.len());
        span.record("bytes", found.bytes);
        span.record("partials_removed", found.partials_removed);
        let coming = u128::from(size) + u128::from(writing);
        let bound = u128::from(self.bound);
        if found.bytes + coming > bound {
            let (evicted, evicted_bytes) =
                evict(&mut found, root, (bound - bound / 4).saturating_sub(coming));
            span.record("evicted", evicted);
            span.record("evicted_bytes", evicted_bytes);
        }
        found
    }

    /// What this cache knows of its directory's size, locked.
    fn usage(&self) -> MutexGuard<'_, Usage> {
        self.usage.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The directory it keeps its entries in (`.silico-cache/` beside a document), as it
    /// was given, with no trailing slash and no `.` components.
    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// Removes every entry in its directory (`cache_clear`, docs/design/m4.1-command-api.md,
    /// section 7 and decision 25): this cache's, those of other secrets, platforms, and
    /// kernels beside them, and none of anything else. An entry is what the cache writes and
    /// [`scan`] counts ([`walk`] finds them for both): a regular file named by
    /// [`ENTRY_DIGITS`] lowercase hex digits in a subdirectory named by
    /// [`SUBDIRECTORY_DIGITS`]. Left are its `.gitignore`, its subdirectories (a write under
    /// way may be about to put its entry in one), partial files (another process's write
    /// under way, whose rename would then fail; a stale one is removed by the next put's
    /// scan), and anything else, under any name; nothing is followed through a link, the
    /// directory's own included (a cache through a link keeps nothing there, so none is
    /// removed), and nothing is removed in a directory that leads out of its workspace as
    /// the clear opens it ([`within`](Self::within)). An entry removed is only a miss: one
    /// another process reads as it's removed is read whole, from the file it holds open, and
    /// one renamed into place after it's listed is left, whole (M2.5's sealed entries: an
    /// entry is whole or absent, never trusted half-written). Read and removed in order of
    /// name, so what it says of a failure is the same each time.
    ///
    /// The directory is opened once, and walked by that handle: each subdirectory opened
    /// from it without following a link, listed, and its entries removed, by its own
    /// handle. A link put in place of the directory, of a subdirectory, or of a directory
    /// above them during the clear (by a process that may write there, as only this user's
    /// may) has the clear go on in the directories it opened, removing nothing where the link
    /// leads. The names alone wouldn't keep it out: a SHA-256 git repository's loose objects
    /// are named as entries are.
    ///
    /// How many entries it removed (not those another process removed first); or, if any
    /// couldn't be removed, or a directory of the cache couldn't be read, each, with how many
    /// it removed of the rest. Its next put scans the directory again, since what this cache
    /// knew of its size is gone.
    pub fn clear(&self) -> Result<u64, PartlyCleared> {
        let span = tracing::info_span!(
            "cache.clear",
            dir = %self.dir.display(),
            removed = tracing::field::Empty,
            failed = tracing::field::Empty,
        );
        let _entered = span.enter();
        let mut clearing = PartlyCleared::default();
        // A directory that's a link (followed nowhere), one that leads out of the workspace
        // as it's opened, a file, or nothing holds no entry of the cache's.
        match self.root(false) {
            Ok(root) => walk(&root, &self.dir, |found| match found {
                Walked::Entry(place, _) => clearing.remove(place),
                Walked::Partial(..) => {}
                Walked::Unread(path, why) => clearing.failed(path, ClearStep::Read, why),
                Walked::Unknown(path, why) => clearing.failed(path, ClearStep::Remove, why),
            }),
            Err(Unopened::Missing | Unopened::Refused | Unopened::NotADirectory(_)) => {}
            Err(Unopened::Failed(e)) => clearing.failed(&self.dir, ClearStep::Read, e),
        }
        // What it knew of the directory's size is gone: its next put scans it.
        self.usage().known = None;
        span.record("removed", clearing.removed);
        span.record("failed", clearing.total);
        if clearing.total == 0 {
            tracing::info!(removed = clearing.removed, "cleared the cache");
            Ok(clearing.removed)
        } else {
            tracing::warn!(
                removed = clearing.removed,
                failed = clearing.total,
                "cleared the cache, but for what couldn't be removed or read"
            );
            Err(clearing)
        }
    }

    /// Whether it has the user's secret to seal entries with: without it, it keeps nothing.
    pub(crate) fn sealed(&self) -> bool {
        self.secret.is_some()
    }

    /// Its directory, opened for one operation (made first if `make` and it isn't there),
    /// or why not: its document's directory opened, through any link on the way; in a
    /// workspace, used only if the system says that handle is in the workspace
    /// ([`within`](Self::within)); and then its own name opened from that handle, never
    /// through a link (said once), made from it first if need be. One open, one question of
    /// where the handle is, and one more open (with, as it's made, a make and an open). In
    /// a workspace, nothing is made by a path: a document's directory that isn't there
    /// (moved, or removed) has a cache that keeps nothing.
    fn root(&self, make: bool) -> Result<OwnedFd, Unopened> {
        let (above, name) = self.parts();
        let above = match at::open_dir_through_links(above) {
            Ok(above) => above,
            // Without a workspace (a tool's cache, anywhere), made with the directories
            // above it, as it always was; a session's document's directory is there.
            Err(Errno::NOENT) if make && self.workspace.is_none() => {
                std::fs::create_dir_all(above).map_err(Unopened::Failed)?;
                at::open_dir_through_links(above).map_err(|e| Unopened::Failed(e.into()))?
            }
            Err(Errno::NOENT) => return Err(Unopened::Missing),
            Err(e) => return Err(Unopened::Failed(e.into())),
        };
        if let Some(workspace) = &self.workspace {
            self.inside(workspace, &above)?;
        }
        match at::open_dir(&above, name) {
            Ok(root) => Ok(root),
            Err(Errno::NOENT) if make => {
                at::make_dir(&above, name).map_err(|e| Unopened::Failed(e.into()))?;
                at::open_dir(&above, name).map_err(|e| self.unopened(&above, name, e))
            }
            Err(e) => Err(self.unopened(&above, name, e)),
        }
    }

    /// Its directory's two parts: the directory it's in, its document's (`.` for a bare
    /// name), and its own name (`.` for a path that has none: `/`, or one ending in `..`).
    fn parts(&self) -> (&Path, &OsStr) {
        match self.dir.file_name() {
            Some(name) => (file::directory(&self.dir), name),
            None => (&self.dir, OsStr::new(".")),
        }
    }

    /// Why its directory, `name` in the directory `above` holds open, couldn't be opened,
    /// for the system's `error`: what's at its name (a link, said once; a file; nothing),
    /// looked at as it is now, from the handle.
    fn unopened(&self, above: &OwnedFd, name: &OsStr, error: Errno) -> Unopened {
        if error == Errno::NOENT {
            return Unopened::Missing;
        }
        match at::stat(above, name) {
            Ok(meta) if meta.is_link() => {
                self.say_linked(&self.dir);
                Unopened::Refused
            }
            Ok(meta) if !meta.is_dir() => Unopened::NotADirectory(error.into()),
            Err(Errno::NOENT) => Unopened::Missing,
            _ => Unopened::Failed(error.into()),
        }
    }

    /// Whether the directory `above` holds open, its document's, is in `workspace` now, by
    /// where the system says the handle is: never by where a path leads, which a link put on
    /// the way since the handle was opened would take elsewhere. One out of the workspace is
    /// refused (said once while it's out); one the system can't place, said.
    fn inside(&self, workspace: &Workspace, above: &OwnedFd) -> Result<(), Unopened> {
        match at::path_of(above) {
            Ok(path)
                if workspace.holds(&path)
                    || self
                        .workspace_as_held
                        .as_ref()
                        .is_some_and(|root| path.starts_with(root)) =>
            {
                // Inside again: should it lead out later, that's said again.
                if self.said_outside.load(Ordering::Relaxed) {
                    self.said_outside.store(false, Ordering::Relaxed);
                }
                Ok(())
            }
            Ok(_) => {
                self.say_outside();
                Err(Unopened::Refused)
            }
            Err(e) => Err(Unopened::Failed(std::io::Error::other(format!(
                "the system couldn't say where its document's directory is ({e})"
            )))),
        }
    }

    /// Its directory, opened for a read or a write ([`root`](Self::root)), or none: what
    /// kept it from being opened is said, but for nothing there, a link, and a directory
    /// that leads out of its workspace, each said already, once, if at all.
    fn opened(&self, make: bool) -> Option<OwnedFd> {
        match self.root(make) {
            Ok(root) => Some(root),
            Err(Unopened::Missing | Unopened::Refused) => None,
            Err(Unopened::NotADirectory(e) | Unopened::Failed(e)) => {
                tracing::warn!(
                    dir = %self.dir.display(),
                    "couldn't open the cache's directory: {e}"
                );
                None
            }
        }
    }

    /// Whether its directory leads out of its workspace now, through a link on its way (its
    /// own name, or a directory above it), by [`file::resolve`]'s one rule, said once if it
    /// does; never without a workspace. One not made yet, inside, doesn't. Asked before a
    /// session's cache is opened (`Registry::cache`); each operation asks its handle again
    /// ([`root`](Self::root)).
    pub(crate) fn leads_out(&self) -> bool {
        let out = self.workspace.as_ref().is_some_and(|workspace| {
            matches!(
                file::resolve(Some(workspace), &self.dir),
                Err(Unresolved::Outside)
            )
        });
        if out {
            self.say_outside();
        }
        out
    }

    /// Says, once while it does, that its directory leads out of its workspace.
    fn say_outside(&self) {
        if !self.said_outside.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                dir = %self.dir.display(),
                "the cache's directory leads out of the workspace, through a link on its \
                 way: keeping no results through it"
            );
        }
    }

    /// Whether the subdirectory `subdirectory` of its directory, which `root` holds open, is
    /// a link, said once.
    fn linked_below(&self, root: &OwnedFd, subdirectory: &str) -> bool {
        let linked = at::is_link(root, subdirectory);
        if linked {
            self.say_linked(&self.dir.join(subdirectory));
        }
        linked
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

    /// Where an entry lives ([`entry_names`]), as a path: to name it by, never to find it.
    fn path(&self, key: &EntryKey) -> PathBuf {
        let (subdirectory, name) = entry_names(key);
        self.dir.join(subdirectory).join(name)
    }
}

/// Why a cache's directory couldn't be opened for an operation.
#[derive(Debug)]
enum Unopened {
    /// Nothing's there.
    Missing,
    /// A directory it doesn't use: a link, or one that leads out of its workspace (each
    /// said once), or one replaced as it was opened.
    Refused,
    /// Something that's not a directory (a file) is there: the system's error.
    NotADirectory(std::io::Error),
    /// It couldn't be opened or looked at: the system's error.
    Failed(std::io::Error),
}

/// Where the entry keyed `key` lives in a cache's directory: under the first two hex digits
/// of its key (so no directory holds every entry), named by the other 62.
fn entry_names(key: &EntryKey) -> (String, String) {
    let mut hex = crate::hex(&key.0);
    let name = hex.split_off(SUBDIRECTORY_DIGITS);
    (hex, name)
}

/// What [`Cache::clear`] did when it couldn't do all of it: how many entries it removed, and
/// each file it couldn't remove and each directory of the cache it couldn't read, the first
/// [`CLEAR_FAILURES_LISTED`] in order of path, with how many there were in all.
#[derive(Debug, Default)]
pub struct PartlyCleared {
    /// The entries it removed.
    pub removed: u64,
    /// The first of what it couldn't do, in the order it met them.
    pub failures: Vec<ClearFailure>,
    /// How many there were, listed or not.
    pub total: u64,
}

/// How many of what [`Cache::clear`] couldn't do are listed: as many as a response lists
/// (`document::PROBLEMS_LISTED`).
pub const CLEAR_FAILURES_LISTED: usize = crate::document::PROBLEMS_LISTED;

/// One thing [`Cache::clear`] couldn't do: where, what, and why.
#[derive(Debug)]
pub struct ClearFailure {
    /// The entry it couldn't remove, or the directory it couldn't read.
    pub path: PathBuf,
    /// Which of the two.
    pub step: ClearStep,
    /// What the system said.
    pub why: std::io::Error,
}

/// What [`Cache::clear`] was doing at a path when it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearStep {
    /// Removing an entry (or reading what it is, first): the entry is left.
    Remove,
    /// Reading a directory of the cache: what's in it, if anything, is left.
    Read,
}

impl PartlyCleared {
    /// `path` counted as where it failed at `step`, for `why`; listed if there's room.
    fn failed(&mut self, path: &Path, step: ClearStep, why: std::io::Error) {
        self.total += 1;
        if self.failures.len() < CLEAR_FAILURES_LISTED {
            self.failures.push(ClearFailure {
                path: path.to_owned(),
                step,
                why,
            });
        }
    }

    /// The entry at `place` removed, and counted, unless another process removed it first.
    fn remove(&mut self, place: Place<'_>) {
        match at::remove(place.dir, place.name) {
            Ok(true) => self.removed += 1,
            Ok(false) => {}
            Err(e) => self.failed(place.path, ClearStep::Remove, e.into()),
        }
    }
}

/// How many lowercase hex digits name a subdirectory of the cache: an entry key's first
/// byte's, so no directory holds every entry.
const SUBDIRECTORY_DIGITS: usize = 2;

/// How many lowercase hex digits name an entry in its subdirectory: the rest of its key's.
const ENTRY_DIGITS: usize = 2 * std::mem::size_of::<EntryKey>() - SUBDIRECTORY_DIGITS;

/// Whether `name` is a subdirectory's of the cache, in which its entries are.
fn is_entry_directory(name: &CStr) -> bool {
    is_hex(name, SUBDIRECTORY_DIGITS)
}

/// Whether `name` is an entry's, in a subdirectory of the cache.
fn is_entry_name(name: &CStr) -> bool {
    is_hex(name, ENTRY_DIGITS)
}

/// Where [`walk`] found a name: the directory of the cache it's in, as the walk holds it
/// open (what's done to it is done there), and that directory's name if it's a
/// subdirectory; the name; and its path (to name it by).
#[derive(Clone, Copy)]
struct Place<'a> {
    dir: BorrowedFd<'a>,
    subdirectory: Option<&'a CStr>,
    name: &'a CStr,
    path: &'a Path,
}

/// What [`walk`] finds in a cache's directory, as it finds it.
enum Walked<'a> {
    /// A partial file's name (a write's, under way or killed), in the directory or a
    /// subdirectory, of whatever type, and its metadata (a link's own).
    Partial(Place<'a>, &'a Meta),
    /// An entry: a regular file named as one in a subdirectory named as one, and its
    /// metadata.
    Entry(Place<'a>, &'a Meta),
    /// A directory of the cache it couldn't read, or list whole (what of it could be listed
    /// is walked), or a name of a subdirectory's whose metadata it couldn't read.
    Unread(&'a Path, std::io::Error),
    /// A name of an entry's whose metadata it couldn't read.
    Unknown(&'a Path, std::io::Error),
}

/// Walks the cache whose directory `root` holds open (at `dir`, to name what it finds by),
/// following no link below it, each directory in order of name: its partial files and its
/// entries, and each directory and name it couldn't read, given to `visit` as it finds them.
/// Each subdirectory is opened from `root`, never through a link, and walked by its own
/// handle (in a span, `cache.walk`), so nothing is found by a path. What has any other name,
/// at either level, is passed over, unread; what's gone as it's read (another process's
/// clear or eviction), or no longer a directory, without a word. The one way the cache's
/// layout is read: [`scan`] and [`Cache::clear`] both walk it.
fn walk(root: &OwnedFd, dir: &Path, mut visit: impl FnMut(Walked<'_>)) {
    for name in names_in(root, dir, &mut visit) {
        let partial = is_partial(&name);
        if !partial && !is_entry_directory(&name) {
            continue;
        }
        let path = dir.join(OsStr::from_bytes(name.to_bytes()));
        let meta = match at::stat(root, name.as_c_str()) {
            Ok(meta) => meta,
            Err(Errno::NOENT) => continue,
            Err(_) if partial => continue,
            Err(e) => {
                visit(Walked::Unread(&path, e.into()));
                continue;
            }
        };
        let place = Place {
            dir: root.as_fd(),
            subdirectory: None,
            name: &name,
            path: &path,
        };
        if partial {
            visit(Walked::Partial(place, &meta));
            continue;
        }
        if !meta.is_dir() {
            // A file or a link at a subdirectory's name: not the cache's.
            continue;
        }
        let _walking = tracing::trace_span!("cache.walk", subdirectory = %path.display()).entered();
        let sub = match at::open_dir(root, name.as_c_str()) {
            Ok(sub) => sub,
            // Gone, or put in its place by a link or a file, since it was looked at.
            Err(Errno::NOENT | Errno::LOOP | Errno::NOTDIR) => continue,
            Err(e) => {
                visit(Walked::Unread(&path, e.into()));
                continue;
            }
        };
        let subdirectory = name;
        for name in names_in(&sub, &path, &mut visit) {
            let partial = is_partial(&name);
            if !partial && !is_entry_name(&name) {
                continue;
            }
            let entry = path.join(OsStr::from_bytes(name.to_bytes()));
            let place = Place {
                dir: sub.as_fd(),
                subdirectory: Some(&subdirectory),
                name: &name,
                path: &entry,
            };
            match at::stat(&sub, name.as_c_str()) {
                Ok(meta) if partial => visit(Walked::Partial(place, &meta)),
                Ok(meta) if meta.is_file() => visit(Walked::Entry(place, &meta)),
                // A link, a directory, a pipe at an entry's name: not one the cache made.
                Ok(_) => {}
                Err(Errno::NOENT) => {}
                Err(_) if partial => {}
                Err(e) => visit(Walked::Unknown(&entry, e.into())),
            }
        }
    }
}

/// The names in the directory `handle` holds open (at `dir`), in order of name; one that
/// can't be read (or listed whole) given to `visit`, as what's in it that can be is listed.
/// Nothing, without a word, for a directory gone (another process's clear, say).
fn names_in(handle: &OwnedFd, dir: &Path, visit: &mut impl FnMut(Walked<'_>)) -> Vec<CString> {
    let (names, unread) = at::names(handle);
    if let Some(e) = unread {
        visit(Walked::Unread(dir, e.into()));
    }
    names
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
    /// Every entry it found.
    entries: Vec<Listed>,
    /// The bytes of its entries and of the partial files it left, by their lengths: wider
    /// than they are, since lengths (of sparse files) can sum past what a `u64` holds.
    bytes: u128,
    /// How many partial files, last written [`STALE_PARTIAL`] ago or more, it removed.
    partials_removed: usize,
    /// Whether it has warned of a file it couldn't remove (once a scan).
    warned: bool,
}

/// An entry a scan found: its last use (its modification time), its path, its
/// subdirectory's name and its own (by which it's found again, from the cache's directory
/// as the scan holds it open, to be evicted), and its size.
struct Listed {
    used: SystemTime,
    path: PathBuf,
    subdirectory: CString,
    name: CString,
    size: u64,
}

/// Lists the entries of the cache whose directory `root` holds open (at `dir`, to name them
/// by), and removes its partial files last written
/// [`STALE_PARTIAL`] or more from `now`. Reads metadata only, and follows no link
/// ([`walk`]): an entry is a regular file named by [`ENTRY_DIGITS`] hex digits in a
/// subdirectory named by [`SUBDIRECTORY_DIGITS`]; a partial is a file named `.partial-…` in
/// either. Anything else is left, and not counted; nor are this process's own partials not
/// yet stale, which its puts count as being written (a process keeps one cache, and its
/// clones, for a directory: two would each miss the other's). What it can't read it passes
/// over.
fn scan(root: &OwnedFd, dir: &Path, now: SystemTime) -> Scan {
    let mut found = Scan {
        entries: Vec::new(),
        bytes: 0,
        partials_removed: 0,
        warned: false,
    };
    let own = format!(".partial-{}-", std::process::id());
    walk(root, dir, |item| match item {
        Walked::Partial(place, meta) => found.remove_or_count_partial(place, meta, now, &own),
        Walked::Entry(place, meta) => {
            found.bytes = found.bytes.saturating_add(u128::from(meta.len()));
            // An entry is always in a subdirectory.
            if let Some(subdirectory) = place.subdirectory {
                found.entries.push(Listed {
                    used: meta.modified(),
                    path: place.path.to_owned(),
                    subdirectory: subdirectory.to_owned(),
                    name: place.name.to_owned(),
                    size: meta.len(),
                });
            }
        }
        Walked::Unread(..) | Walked::Unknown(..) => {}
    });
    found
}

impl Scan {
    /// Removes the partial file at `place` if it was last written [`STALE_PARTIAL`] or more
    /// from `now`, before or after (a link that's named as one, by its own time: never what
    /// it points at); otherwise counts it, unless it's this process's (named from `own`).
    fn remove_or_count_partial(
        &mut self,
        place: Place<'_>,
        meta: &Meta,
        now: SystemTime,
        own: &str,
    ) {
        let written = meta.modified();
        let apart = match now.duration_since(written) {
            Ok(age) => age,
            Err(ahead) => ahead.duration(),
        };
        if apart >= STALE_PARTIAL
            && !meta.is_dir()
            && self.remove(place.dir, place.name, place.path)
        {
            self.partials_removed += 1;
        } else if meta.is_file() && !place.name.to_bytes().starts_with(own.as_bytes()) {
            self.bytes = self.bytes.saturating_add(u128::from(meta.len()));
        }
    }

    /// Removes the file (or link) `name` in `dir`, at `path`, warning of a failure (once a
    /// scan): whether it's gone, as it is if another session removed it first.
    fn remove(&mut self, dir: BorrowedFd<'_>, name: &CStr, path: &Path) -> bool {
        match at::remove(dir, name) {
            Ok(_) => true,
            Err(e) => {
                self.warn(path, e);
                false
            }
        }
    }

    /// Warns that the file at `path` couldn't be removed, for `e`, unless it has this scan.
    fn warn(&mut self, path: &Path, e: Errno) {
        if !std::mem::replace(&mut self.warned, true) {
            tracing::warn!(path = %path.display(), "couldn't remove a cache file: {e}");
        }
    }

    /// Evicts the entry `listed`, found again from `root` (its subdirectory opened from it,
    /// never through a link, unless it's the one `open` holds already): whether it's gone,
    /// as it is if another session removed it, or its subdirectory, first.
    fn evict(
        &mut self,
        root: &OwnedFd,
        listed: &Listed,
        open: &mut Option<(CString, OwnedFd)>,
    ) -> bool {
        if open
            .as_ref()
            .is_none_or(|(name, _)| *name != listed.subdirectory)
        {
            *open = match at::open_dir(root, listed.subdirectory.as_c_str()) {
                Ok(dir) => Some((listed.subdirectory.clone(), dir)),
                Err(Errno::NOENT) => return true,
                Err(e) => {
                    self.warn(&listed.path, e);
                    return false;
                }
            };
        }
        match open {
            Some((_, dir)) => self.remove(dir.as_fd(), &listed.name, &listed.path),
            None => false,
        }
    }
}

/// Evicts `found`'s entries, least recently used first (by path, of two used at once),
/// from the cache's directory `root` holds open, until its bytes are at most `target` or no
/// entry is left. Returns how many it evicted, and their bytes.
fn evict(found: &mut Scan, root: &OwnedFd, target: u128) -> (usize, u128) {
    let mut entries = std::mem::take(&mut found.entries);
    entries.sort_by(|a, b| (a.used, &a.path).cmp(&(b.used, &b.path)));
    let (mut evicted, mut evicted_bytes) = (0, 0_u128);
    let mut kept = Vec::new();
    let mut open = None;
    for listed in entries {
        // Gone already counts as evicted: another session evicted it, or rebuilt it after a
        // bad read.
        if found.bytes > target && found.evict(root, &listed, &mut open) {
            found.bytes = found.bytes.saturating_sub(u128::from(listed.size));
            evicted += 1;
            evicted_bytes = evicted_bytes.saturating_add(u128::from(listed.size));
        } else {
            kept.push(listed);
        }
    }
    found.entries = kept;
    (evicted, evicted_bytes)
}

/// Whether `name` is a partial file's: one [`new_file`] makes.
fn is_partial(name: &CStr) -> bool {
    name.to_bytes().starts_with(b".partial-")
}

/// Whether `name` is `digits` lowercase hex digits, as an entry's path is.
fn is_hex(name: &CStr, digits: usize) -> bool {
    let name = name.to_bytes();
    name.len() == digits && name.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Marks the entry `name` in `dir` as used now, so it's evicted after those used before it.
/// A hit whose time can't be set (a read-only cache, say) is still a hit.
fn renew(dir: &OwnedFd, name: &str) {
    if let Ok(file) = at::open_to_read(dir, name) {
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
        match read_file(CWD, file) {
            FileRead::Bytes(bytes) => {
                return <[u8; 32]>::try_from(bytes.as_slice())
                    .map_err(|_| std::io::Error::other("it isn't 32 bytes"));
            }
            FileRead::Missing => {}
            FileRead::Unusable(why) => return Err(std::io::Error::other(format!("it {why}"))),
            FileRead::Failed(e) => return Err(e),
        }
        let secret: [u8; 32] = crate::system_random()?;
        let dir = file
            .parent()
            .ok_or_else(|| std::io::Error::other("it has no directory"))?;
        std::fs::create_dir_all(dir)?;
        // Through any link on the way, the user's own: their cache directory, which they may
        // have made one (to another disk, say), where no one else may write.
        let dir = at::open_dir_through_links(dir)?;
        let (partial, mut out) = at::new_file(&dir, Mode::from_raw_mode(0o600))?;
        // Durable before it's linked: a secret lost after entries were sealed with it
        // would leave a damaged file in its place.
        let written = out.write_all(&secret).and_then(|()| out.sync_all());
        // Linked, not renamed, to its name: two sessions making it at once agree on one.
        let linked = written
            .and_then(|()| at::link(&dir, partial.as_str(), file).map_err(std::io::Error::from));
        let _ = at::remove(&dir, partial.as_str());
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

/// The bytes of the regular file `name` in `dir` (or at the path `name`, from [`CWD`]),
/// read without following a symbolic link or waiting on a pipe, and no more than
/// [`MAX_ENTRY`] of them.
fn read_file(dir: impl AsFd, name: impl rustix::path::Arg) -> FileRead {
    let file = match at::open_to_read(dir, name) {
        Ok(file) => file,
        Err(Errno::NOENT) => return FileRead::Missing,
        Err(Errno::LOOP) => return FileRead::Unusable("is a symbolic link"),
        Err(e) => return FileRead::Failed(e.into()),
    };
    match file.metadata() {
        Ok(meta) if !meta.is_file() => return FileRead::Unusable("isn't a file"),
        Ok(meta) if meta.len() > MAX_ENTRY => return FileRead::Unusable("is too large"),
        Ok(_) => {}
        Err(e) => return FileRead::Failed(e),
    }
    match read_at_most(file, MAX_ENTRY) {
        Ok(Some(bytes)) => FileRead::Bytes(bytes),
        // Grown past the limit since it was measured.
        Ok(None) => FileRead::Unusable("is too large"),
        Err(e) => FileRead::Failed(e),
    }
}

/// All of `reader`, if it holds at most `limit` bytes; none if it holds more, read no further
/// than one past the limit.
fn read_at_most(reader: impl std::io::Read, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
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
/// measure to make again, a description a worker couldn't send (a value out of range, or
/// more or fewer of a kind than the summary counts) a description to ask for again, and a
/// refusal is worded now, by this core, and kept only if the cache would keep it (this core
/// may judge it otherwise than the one that wrote it).
fn built_of(entry: Entry) -> Built {
    let shape = ShapeData::checked(Blob::inline(entry.brep), entry.valid);
    match entry.measure {
        Measure::Measured {
            summary,
            description,
        } if crate::kernel::summary_fault(&summary).is_none() => {
            let built = Built::new(shape, entry.history, Some(summary.clone()));
            match description {
                KeptDescription::Described(description)
                    if description.validate(1).is_ok()
                        && crate::kernel::description_fault(&summary, &description).is_none() =>
                {
                    built.with_described(Described::Whole(description))
                }
                KeptDescription::Refused { op, failure } => {
                    let refused = crate::worker::failure_error(&op, failure);
                    if lasting(&refused) {
                        built.with_described(Described::Refused(refused))
                    } else {
                        built
                    }
                }
                KeptDescription::Unknown | KeptDescription::Described(_) => built,
            }
        }
        Measure::Refused { op, failure } => {
            let refused = crate::worker::failure_error(&op, failure);
            if lasting(&refused) {
                Built::refused_measure(shape, entry.history, refused)
            } else {
                Built::new(shape, entry.history, None)
            }
        }
        Measure::Unmeasured | Measure::Measured { .. } => Built::new(shape, entry.history, None),
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

/// Writes an entry named `name` in the subdirectory `subdirectory` of the cache's directory,
/// which `root` holds open (at `dir`, to name its `.gitignore` by), whole or not at all: to
/// a file of its own beside it, then renamed over it, so a reader never sees a part of one,
/// and two writers of the same entry (two sessions) leave one or the other. Every step is
/// taken from `root`: the subdirectory made in it, and opened from it without following a
/// link. Not synced to the disk: an entry a crash cuts short or empties doesn't hold
/// together, so it's a miss. Its modification time, its last use, is set to now, to the
/// nanosecond (Linux's own moves only once a tick).
fn write_entry(
    root: &OwnedFd,
    dir: &Path,
    subdirectory: &str,
    name: &str,
    seal: &Seal,
    payload: &[u8],
) -> std::io::Result<()> {
    at::make_dir(root, subdirectory)?;
    let sub = match at::open_dir(root, subdirectory) {
        Ok(sub) => sub,
        Err(_) if at::is_link(root, subdirectory) => {
            return Err(std::io::Error::other("its directory is a link"));
        }
        Err(e) => return Err(e.into()),
    };
    ignore_itself(root, dir);
    let (partial, mut file) = at::new_file(&sub, Mode::from_raw_mode(0o644))?;
    let written = file
        .write_all(MAGIC)
        .and_then(|()| file.write_all(&seal.0))
        .and_then(|()| file.write_all(payload));
    // Where the file system won't take a time (vfat, some network shares), it keeps its
    // own: the entry is still kept.
    let _ = file.set_modified(SystemTime::now());
    let renamed =
        written.and_then(|()| at::rename(&sub, partial.as_str(), name).map_err(Into::into));
    if renamed.is_err() {
        let _ = at::remove(&sub, partial.as_str());
    }
    renamed
}

/// Gives the cache's directory, which `root` holds open (at `dir`, to name it by), its
/// `.gitignore`, unless it has it: written whole, over anything else at its name (a file
/// emptied by a failed write, or one that came with a clone), as an entry is.
fn ignore_itself(root: &OwnedFd, dir: &Path) {
    if let FileRead::Bytes(bytes) = read_file(root, ".gitignore")
        && bytes == GITIGNORE
    {
        return;
    }
    let written = at::new_file(root, Mode::from_raw_mode(0o644))
        .map_err(std::io::Error::from)
        .and_then(|(partial, mut file)| {
            let renamed = file.write_all(GITIGNORE).and_then(|()| {
                at::rename(root, partial.as_str(), ".gitignore").map_err(Into::into)
            });
            if renamed.is_err() {
                let _ = at::remove(root, partial.as_str());
            }
            renamed
        });
    if let Err(e) = written {
        let path = dir.join(".gitignore");
        tracing::warn!(path = %path.display(), "couldn't write the cache's .gitignore: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clear_lists_the_first_hundred_of_what_it_couldnt_do_and_counts_them_all() {
        // The design's section 7: the first hundred listed, all counted. 101 met: the
        // first hundred, in the order met, and 101 in all.
        let mut partly = PartlyCleared::default();
        for n in 0..101 {
            partly.failed(
                &PathBuf::from(format!("/cache/{n:03}")),
                ClearStep::Remove,
                std::io::Error::from_raw_os_error(libc::EACCES),
            );
        }
        assert_eq!(partly.failures.len(), 100);
        assert_eq!(partly.total, 101);
        let listed: Vec<PathBuf> = partly.failures.iter().map(|f| f.path.clone()).collect();
        let first: Vec<PathBuf> = (0..100)
            .map(|n| PathBuf::from(format!("/cache/{n:03}")))
            .collect();
        assert_eq!(listed, first);
        assert_eq!(partly.removed, 0);
    }

    #[test]
    fn an_entrys_path_is_its_keys_first_two_hex_digits_then_the_other_62() {
        // The layout scan and clear walk: 32 bytes of key, 64 hex digits, two for the
        // subdirectory and 62 for the name.
        let cache = Cache::unsealed("/cache");
        let mut key = [0_u8; 32];
        key[0] = 0xab;
        key[31] = 0x01;
        let path = cache.path(&EntryKey(key));
        let name = format!("{}01", "0".repeat(60));
        assert_eq!(path, Path::new("/cache/ab").join(&name));
        assert!(is_entry_directory(c"ab"));
        assert!(is_entry_name(&CString::new(name.clone()).unwrap()));
        assert!(!is_entry_name(&CString::new(&name[1..]).unwrap()));
    }

    /// The directory of `cache`, opened for an operation (made if need be).
    fn root(cache: &Cache) -> OwnedFd {
        cache.root(true).unwrap()
    }

    /// The directory at `path`, opened as the cache opens its own.
    fn opened_at(path: &Path) -> OwnedFd {
        at::open_dir(CWD, path).unwrap()
    }

    /// Counts the warnings logged where it's the subscriber.
    struct Warnings(Arc<std::sync::Mutex<u32>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Warnings {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() == tracing::Level::WARN {
                *self.0.lock().unwrap() += 1;
            }
        }
    }

    /// A subscriber that counts the warnings logged under it, and its count.
    fn counting_warnings() -> (impl tracing::Subscriber, Arc<std::sync::Mutex<u32>>) {
        use tracing_subscriber::layer::SubscriberExt as _;
        let warnings = Arc::new(std::sync::Mutex::new(0));
        let subscriber = tracing_subscriber::registry().with(Warnings(Arc::clone(&warnings)));
        (subscriber, warnings)
    }

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
        assert_eq!(cache.make_room(&root(&cache), 50), Room::Busy);
        let usage = cache.usage();
        assert_eq!((usage.known, usage.writing), (Some(60), 60));
        drop(usage);
        // 60 + 40 is exactly 100: kept, and counted.
        assert_eq!(cache.make_room(&root(&cache), 40), Room::Made);
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
        let room = cache.make_room_scanning(&root(&cache), 30, |cache, root, size, writing| {
            let left = cache.scan_and_evict(root, size, writing);
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
        let room = cache.make_room_scanning(&root(&cache), 30, |cache, root, size, writing| {
            let left = cache.scan_and_evict(root, size, writing);
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
        let room = cache.make_room_scanning(&root(&cache), 30, |cache, root, size, writing| {
            let left = cache.scan_and_evict(root, size, writing);
            scans += 1;
            if scans == 1 {
                assert_eq!(cache.make_room(root, 40), Room::Made);
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
        assert_eq!(cache.make_room(&root(&cache), 40), Room::Made);
        let usage = cache.usage();
        assert_eq!((usage.known, usage.scans), (Some(800), 0));
        drop(usage);
        let other =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(800);
        other.usage().known = Some(760);
        assert_eq!(other.make_room(&root(&other), 41), Room::Made);
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
        assert_eq!(cache.make_room(&root(&cache), 10), Room::Made);
        assert_eq!(cache.usage().known, Some(10));
        let theirs = sub.join(".partial-1-0");
        std::fs::write(&theirs, vec![0; 200]).unwrap();
        let fresh =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(100);
        assert_eq!(fresh.make_room(&root(&fresh), 10), Room::Stuck);
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
            assert_eq!(cache.make_room(&root(&cache), 100), Room::Made);
        }
        assert_eq!(cache.usage().scans, 1);
        // A bound of 800, an eighth of 100: puts of 50 scan at the first (50 since), not
        // at the next (exactly 100), and when 50 more would make 150: the 3rd, 5th, 7th,
        // and 9th.
        let cache =
            Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32]))).with_bound(800);
        for _ in 0..9 {
            assert_eq!(cache.make_room(&root(&cache), 50), Room::Made);
        }
        assert_eq!(cache.usage().scans, 5);
        // A clone shares what its cache knows (50 since): no scan of its own.
        let twin = cache.clone();
        assert_eq!(twin.make_room(&root(&twin), 10), Room::Made);
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
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cache.make_room(&root(&cache), 10)
            }))
        });
        assert!(unwound.is_err());
        assert!(!cache.usage().scanning);
        assert_eq!(cache.make_room(&root(&cache), 10), Room::Made);
        assert_eq!(cache.usage().scans, 2);
    }

    #[test]
    fn a_full_cache_says_so_once_for_itself_and_its_clones() {
        // Found by review: saying it at every refusal but the first passed every test.
        let scratch = Scratch::new("full");
        let cache = Cache::new(scratch.0.clone(), this_platform(), Some(Secret([7; 32])));
        let (subscriber, warnings) = counting_warnings();
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
        let scratch = Scratch::new("evict-sparse");
        let half = 1_u64 << 63;
        let gone = |n: u8| {
            scratch
                .0
                .join(format!("0{n}"))
                .join("0".repeat(ENTRY_DIGITS))
        };
        let mut scan = Scan {
            entries: (0..3)
                .map(|n| Listed {
                    used: SystemTime::UNIX_EPOCH,
                    path: gone(n),
                    subdirectory: CString::new(format!("0{n}")).unwrap(),
                    name: CString::new("0".repeat(ENTRY_DIGITS)).unwrap(),
                    size: half,
                })
                .collect(),
            bytes: 3 * u128::from(half),
            partials_removed: 0,
            warned: false,
        };
        let root = opened_at(&scratch.0);
        assert_eq!(evict(&mut scan, &root, u128::from(half)), (2, 1_u128 << 64));
        assert_eq!(scan.bytes, u128::from(half));
        // What's left is the newest.
        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].path, gone(2));
    }

    #[test]
    fn a_read_takes_up_to_its_limit_and_refuses_more_however_much_more() {
        // Found by the mutation run: an entry grown past the limit between measuring and
        // reading it was untestable without a race. An endless reader is more than any limit.
        assert_eq!(read_at_most(std::io::repeat(7), 8).unwrap(), None);
        assert_eq!(read_at_most(&[7u8; 9][..], 8).unwrap(), None);
        assert_eq!(read_at_most(&[7u8; 8][..], 8).unwrap(), Some(vec![7; 8]));
        assert_eq!(read_at_most(&[][..], 8).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn a_scan_warns_of_the_first_file_it_couldnt_remove_and_only_that() {
        // Found by the mutation run: warning at every failure but the first passed every
        // test (two failures warn once either way). A name through a regular file can't be
        // removed (ENOTDIR, on Linux and macOS, as root too): its first failure is warned
        // of at once, and the two after it aren't.
        let scratch = Scratch::new("remove-warns");
        let file = scratch.0.join("file");
        std::fs::write(&file, b"x").unwrap();
        let through = file.join("entry");
        let dir = opened_at(&scratch.0);
        let remove =
            |scan: &mut Scan, name: &CStr, path: &Path| scan.remove(dir.as_fd(), name, path);
        let mut scan = Scan {
            entries: Vec::new(),
            bytes: 0,
            partials_removed: 0,
            warned: false,
        };
        let (subscriber, warnings) = counting_warnings();
        tracing::subscriber::with_default(subscriber, || {
            assert!(!remove(&mut scan, c"file/entry", &through));
            assert_eq!(
                *warnings.lock().unwrap(),
                1,
                "the first failure is warned of"
            );
            assert!(!remove(&mut scan, c"file/entry", &through));
            assert!(!remove(&mut scan, c"file/entry", &through));
            // Gone already counts as removed, and isn't warned of.
            assert!(remove(&mut scan, c"none", &scratch.0.join("none")));
        });
        assert_eq!(*warnings.lock().unwrap(), 1);
        assert!(scan.warned);
        assert!(file.exists());
    }

    #[test]
    fn a_cache_through_a_link_says_so_once_for_itself_and_its_clones() {
        // Found by the milestone's mutation run: saying it never, or every time but the
        // first, passed every test.
        let scratch = Scratch::new("said-once");
        let real = scratch.0.join("real");
        std::fs::create_dir(&real).unwrap();
        let linked = scratch.0.join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let cache = Cache::new(linked, this_platform(), Some(Secret([7; 32])));
        let (subscriber, warnings) = counting_warnings();
        tracing::subscriber::with_default(subscriber, || {
            assert!(cache.opened(false).is_none());
            assert!(cache.opened(true).is_none());
            assert!(cache.clone().opened(false).is_none());
        });
        assert_eq!(*warnings.lock().unwrap(), 1);
    }

    #[test]
    fn a_cache_directory_leads_out_only_through_a_link_out_of_its_workspace() {
        // Moved from `cache_clear`'s own check (#183), which this one replaces. A workspace,
        // `work`, and a directory beside it, `outside`.
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        let work = real.join("work");
        let outside = real.join("outside");
        std::fs::create_dir_all(work.join("plain")).unwrap();
        std::fs::create_dir_all(outside.join("cache")).unwrap();
        let workspace = Workspace::new(&work).unwrap();
        let within = |dir: &Path| Cache::unsealed(dir).within(workspace.clone());
        // Inside, there or not yet made.
        std::fs::create_dir(work.join("plain/.silico-cache")).unwrap();
        assert!(!within(&work.join("plain/.silico-cache")).leads_out());
        assert!(!within(&work.join(".silico-cache")).leads_out());
        // A link out, at `.silico-cache` or at a directory above it.
        std::os::unix::fs::symlink(outside.join("cache"), work.join(".silico-cache")).unwrap();
        assert!(within(&work.join(".silico-cache")).leads_out());
        std::os::unix::fs::symlink(&outside, work.join("away")).unwrap();
        assert!(within(&work.join("away/.silico-cache")).leads_out());
        // A link to a directory inside is no way out.
        std::os::unix::fs::symlink(work.join("plain"), work.join("near")).unwrap();
        assert!(!within(&work.join("near/.silico-cache")).leads_out());
        // Without a workspace, nothing leads out.
        assert!(!Cache::unsealed(work.join("away/.silico-cache")).leads_out());
    }

    #[test]
    fn a_cache_that_leads_out_of_its_workspace_keeps_nothing_and_says_so_once() {
        // Its document's directory swapped for a link out after it was made: its directory
        // isn't opened, which is said once for itself and its clones; put back inside, it's
        // opened again, and should it lead out again, that's said again. Without a
        // workspace, the same directory is used.
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        let (work, outside) = (real.join("work"), real.join("outside"));
        std::fs::create_dir_all(work.join("sub/.silico-cache")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let cache_dir = work.join("sub/.silico-cache");
        let cache = Cache::new(cache_dir.clone(), this_platform(), Some(Secret([7; 32])))
            .within(Workspace::new(&work).unwrap());
        let anywhere = Cache::new(cache_dir, this_platform(), Some(Secret([7; 32])));
        assert!(cache.opened(false).is_some());
        let swap_out = || {
            std::fs::rename(work.join("sub"), outside.join("sub")).unwrap();
            std::os::unix::fs::symlink(outside.join("sub"), work.join("sub")).unwrap();
        };
        let put_back = || {
            std::fs::remove_file(work.join("sub")).unwrap();
            std::fs::rename(outside.join("sub"), work.join("sub")).unwrap();
        };
        swap_out();
        let (subscriber, warnings) = counting_warnings();
        tracing::subscriber::with_default(subscriber, || {
            assert!(cache.opened(false).is_none());
            assert!(cache.opened(true).is_none());
            assert!(cache.clone().opened(false).is_none());
            assert!(anywhere.opened(false).is_some());
        });
        assert_eq!(*warnings.lock().unwrap(), 1);
        put_back();
        assert!(cache.opened(false).is_some());
        swap_out();
        let (subscriber, warnings) = counting_warnings();
        tracing::subscriber::with_default(subscriber, || {
            assert!(cache.opened(false).is_none());
            assert!(cache.opened(false).is_none());
        });
        assert_eq!(*warnings.lock().unwrap(), 1);
    }

    #[test]
    fn a_documents_directory_moved_out_between_its_open_and_its_check_isnt_used() {
        // A move after the document's directory is opened and before it's checked: the
        // handle opened on `work/sub`, then that directory moved out of the workspace and
        // another made at its name. The path leads inside, to the new one, but the handle
        // is where the system says it is, `outside/sub`, so it isn't used, which is said;
        // one opened on the directory there now is.
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (work, outside) = (root.join("work"), root.join("outside"));
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let workspace = Workspace::new(&work).unwrap();
        let cache = Cache::new(
            work.join("sub/.silico-cache"),
            this_platform(),
            Some(Secret([7; 32])),
        )
        .within(workspace.clone());
        let first = opened_at(&work.join("sub"));
        std::fs::rename(work.join("sub"), outside.join("sub")).unwrap();
        std::fs::create_dir(work.join("sub")).unwrap();
        let (subscriber, warnings) = counting_warnings();
        tracing::subscriber::with_default(subscriber, || {
            assert!(matches!(
                cache.inside(&workspace, &first),
                Err(Unopened::Refused)
            ));
            assert!(
                cache
                    .inside(&workspace, &opened_at(&work.join("sub")))
                    .is_ok()
            );
        });
        assert_eq!(*warnings.lock().unwrap(), 1);
    }

    #[test]
    fn a_documents_directory_reached_through_a_link_inside_its_workspace_is_used() {
        // `work/near` a link to `work/real`: the cache at `work/near/.silico-cache` is made,
        // from the handle, in `work/real`, and used; the same through a link out isn't, and
        // nothing is made there.
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (work, outside) = (root.join("work"), root.join("outside"));
        std::fs::create_dir_all(work.join("real")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink("real", work.join("near")).unwrap();
        std::os::unix::fs::symlink(&outside, work.join("away")).unwrap();
        let workspace = Workspace::new(&work).unwrap();
        let within = |dir: PathBuf| {
            Cache::new(dir, this_platform(), Some(Secret([7; 32]))).within(workspace.clone())
        };
        assert!(within(work.join("near/.silico-cache")).root(true).is_ok());
        assert!(work.join("real/.silico-cache").is_dir());
        assert!(matches!(
            within(work.join("away/.silico-cache")).root(true),
            Err(Unopened::Refused)
        ));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
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
        assert!(matches!(read_file(CWD, &file), FileRead::Bytes(b) if b == b"an entry"));
        assert!(matches!(
            read_file(CWD, scratch.0.join("none").as_path()),
            FileRead::Missing
        ));
        // A link isn't followed, even to a regular file.
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(matches!(
            read_file(CWD, &link),
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
            read_file(CWD, &fifo),
            FileRead::Unusable("isn't a file")
        ));
        assert!(matches!(
            read_file(CWD, &scratch.0),
            FileRead::Unusable("isn't a file")
        ));
        // A path through a file (ENOTDIR) is neither missing nor a link: a failure.
        assert!(matches!(
            read_file(CWD, file.join("below").as_path()),
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
        match read_file(CWD, &file) {
            FileRead::Bytes(bytes) => assert_eq!(bytes.len() as u64, MAX_ENTRY),
            FileRead::Missing => panic!("missing"),
            FileRead::Unusable(why) => panic!("unusable: {why}"),
            FileRead::Failed(e) => panic!("failed: {e}"),
        }
        f.set_len(MAX_ENTRY + 1).unwrap();
        assert!(matches!(
            read_file(CWD, &file),
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
        let read = read_file(CWD, &file);
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
        let root = opened_at(&scratch.0);
        let error =
            write_entry(&root, &scratch.0, "ab", "cdef", &Seal([0; 32]), b"payload").unwrap_err();
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
        let dir = opened_at(&scratch.0);
        let mode = Mode::from_raw_mode(0o600);
        let (first, _) = at::new_file(&dir, mode).unwrap();
        let prefix = format!(".partial-{pid}-");
        let n: u64 = first.strip_prefix(&prefix).unwrap().parse().unwrap();
        for taken in n + 1..=n + 8 {
            std::fs::write(named(taken), b"planted").unwrap();
        }
        let (ninth, _) = at::new_file(&dir, mode).unwrap();
        assert_eq!(scratch.0.join(ninth), named(n + 9));
        for taken in n + 10..=n + 18 {
            std::fs::write(named(taken), b"planted").unwrap();
        }
        let error = at::new_file(&dir, mode).unwrap_err();
        assert_eq!(error, Errno::EXIST, "{error}");
        assert!(!named(n + 19).exists());
        // Planted names are left as they were.
        for taken in (n + 1..=n + 8).chain(n + 10..=n + 18) {
            assert_eq!(std::fs::read(named(taken)).unwrap(), b"planted");
        }
    }

    /// What [`walk`] gave its visitor, as data: what it found, and where.
    fn walked(found: Walked<'_>) -> (&'static str, PathBuf) {
        match found {
            Walked::Partial(place, _) => ("partial", place.path.to_owned()),
            Walked::Entry(place, _) => ("entry", place.path.to_owned()),
            Walked::Unread(path, _) => ("unread", path.to_owned()),
            Walked::Unknown(path, _) => ("unknown", path.to_owned()),
        }
    }

    #[test]
    fn a_walk_of_a_cache_directory_gone_finds_nothing_and_says_nothing() {
        // Another process's clear (or the user) removed the directory once it was opened,
        // before it's listed: nothing in it, and nothing it couldn't read.
        let scratch = Scratch::new("walk-gone");
        let gone = scratch.0.join("cache");
        std::fs::create_dir(&gone).unwrap();
        let root = opened_at(&gone);
        std::fs::remove_dir(&gone).unwrap();
        let mut seen = Vec::new();
        walk(&root, &gone, |found| seen.push(walked(found)));
        assert_eq!(seen, []);
    }

    #[test]
    fn a_walk_passes_over_an_entry_or_a_subdirectory_removed_after_its_listed() {
        // Another process's clear or eviction removes what this walk has listed but not
        // yet looked at: an entry beside the one it's at, and the subdirectory after it.
        // The visitor removes both as it's given the first entry, after both were listed
        // (`00` with its two names, before either is looked at; `01` before `00` is walked):
        // each is passed over without a word, as if it had never been there.
        let scratch = Scratch::new("walk-raced");
        let entry = |sub: &str, byte: char| {
            scratch
                .0
                .join(sub)
                .join(byte.to_string().repeat(ENTRY_DIGITS))
        };
        let (first, second, later) = (entry("00", 'a'), entry("00", 'b'), entry("01", 'c'));
        for path in [&first, &second, &later] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"an entry").unwrap();
        }
        let mut seen = Vec::new();
        let root = opened_at(&scratch.0);
        walk(&root, &scratch.0, |found| {
            if matches!(&found, Walked::Entry(place, _) if place.path == first.as_path()) {
                std::fs::remove_file(&second).unwrap();
                std::fs::remove_dir_all(scratch.0.join("01")).unwrap();
            }
            seen.push(walked(found));
        });
        assert_eq!(seen, [("entry", first.clone())]);
        assert!(first.is_file() && !second.exists() && !later.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_workspace_spelled_through_the_data_volume_keeps_its_cache() {
        // From the review (round 3): macOS names a handle on `/System/Volumes/Data/Users/...`
        // as `/Users/...` (its firmlink), where `realpath` keeps the spelling given; compared
        // with the canonical root alone, every operation of such a workspace's cache was
        // refused as out of it. The root as the system names its own handle is taken too.
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let Ok(rest) = root.strip_prefix("/") else {
            panic!("{root:?} isn't from the top");
        };
        let data = PathBuf::from("/System/Volumes/Data").join(rest);
        std::fs::create_dir_all(data.join("work/sub")).unwrap();
        let workspace = Workspace::new(&data.join("work")).unwrap();
        let cache = Cache::new(
            data.join("work/sub/.silico-cache"),
            this_platform(),
            Some(Secret([7; 32])),
        )
        .within(workspace);
        assert!(!cache.leads_out());
        let opened = cache.root(true);
        assert!(opened.is_ok(), "{:?}", opened.err());
    }

    #[test]
    fn a_directorys_names_are_listed_in_order_of_name_whatever_order_they_were_made_in() {
        // Sixty-four names, `00` to `3f`, made in a shuffled order, so a file system that
        // lists in the order names were made (or the reverse, or by a hash of each, as APFS
        // and ext4 do) lists them otherwise than sorted: checked first, so the test can
        // fail. Listed by `at::names`, they're in order of name, written out by hand.
        let scratch = Scratch::new("names-sorted");
        let sorted: Vec<String> = (0..64).map(|i| format!("{i:02x}")).collect();
        // Made in a fixed shuffle (37 is prime to 64, so i * 37 mod 64 visits each once), not
        // last first: a file system that lists newest first (Linux's tmpfs before 6.6) would
        // list names made last first sorted already (the Linux gate, round 3).
        for i in 0..64 {
            let name = &sorted[(i * 37) % 64];
            std::fs::write(scratch.0.join(name), b"").unwrap();
        }
        let raw: Vec<String> = std::fs::read_dir(&scratch.0)
            .unwrap()
            .map(|item| item.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(raw.len(), 64);
        assert_ne!(
            raw, sorted,
            "the file system lists in order of name already"
        );
        let (names, unread) = at::names(opened_at(&scratch.0));
        assert!(unread.is_none(), "{unread:?}");
        let names: Vec<&str> = names.iter().map(|name| name.to_str().unwrap()).collect();
        assert_eq!(names[..4], ["00", "01", "02", "03"]);
        assert_eq!(names[60..], ["3c", "3d", "3e", "3f"]);
        assert_eq!(names, sorted);
    }
}
