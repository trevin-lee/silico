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

use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("dir", &self.dir)
            .field("platform", &self.platform)
            .field("sealed", &self.secret.is_some())
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
        }
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
            Some(entry) => Some(Built::new(
                ShapeData::checked(Blob::inline(entry.brep), entry.valid),
                entry.history,
                entry.summary,
            )),
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
        if let Err(e) = write_entry(&self.dir, &path, &seal, &payload) {
            tracing::warn!(path = %path.display(), "couldn't write a cache entry: {e}");
        }
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
/// an entry a crash cuts short or empties doesn't hold together, so it's a miss.
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
    fn a_cache_file_is_read_only_if_its_a_regular_file() {
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
