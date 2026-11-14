//! Whether a session's file changed on disk (docs/design/m5.0-app.md, section 4, *A file
//! changed on disk*; `status`'s `disk`, M5.4; decisions 1 and 3, pending the maintainer).
//! Below any command:
//!
//! - **Off the caller's thread.** [`DiskWatch::finding`] answers from the last comparison,
//!   and starts the next on a thread of its own if none is running and the last ended at
//!   least the watch's interval ago ([`DISK_CHECK_INTERVAL`]). It never waits on the file.
//! - **By bytes.** A comparison reads the file whole, as the session reads a document
//!   ([`crate::file::read`]: a regular file, at most what Silico reads of one), and compares
//!   it with the bytes the session last read or wrote, not times (decision 2).
//! - **By the session's counter.** Each comparison records which of the session's reads and
//!   writes it compared against, by a counter [`DiskWatch::held`] advances at each one, never
//!   by the bytes: two of the session's own saves, A then B then A again, would match by
//!   bytes. A result whose counter is no longer the session's (its own save landed
//!   meanwhile) is dropped, never reported. A save is bracketed: [`DiskWatch::saving`]
//!   before it writes gives a [`Saving`], which keeps every finding back and starts no
//!   comparison until it's [`saved`](Saving::saved) with what was written, or dropped (the
//!   save failed: the finding unknown until the next comparison). So the session's own
//!   save never reads as a change on disk, whenever a comparison reads the file.
//! - **Unchanged after the session's own read or write.** It holds those bytes:
//!   [`DiskWatch::held`] and [`Saving::saved`] find the file unchanged at once, and a
//!   later change on disk wakes the client again.
//! - **Inside the workspace.** The path is resolved in the session's workspace before
//!   each read, as the session resolves it to reload: one that leads out reads as
//!   unreadable, never saying what's there.
//!
//! A read that hangs (a stalled network mount) holds the watch's one comparison: its last
//! finding stands until the read ends, and the read is logged, once, as a warning, once it
//! has run [`DISK_READ_STALLED`]. Its thread isn't stopped: it ends with the read, holding the
//! session's bytes until then, after the session has closed if it closes first.
//!
//! An outside write that lands between the session's own read or save and the watch being
//! told of it ([`DiskWatch::held`], [`Saving::saved`]) reads as unchanged until the file
//! changes again; the session's next command, which reads the file itself, still finds it.
//!
//! It reloads nothing: what to do with a finding is the session's (A3), told of a change only
//! once it has settled ([`DiskWatch::when_changed`]): once two comparisons in a row, against
//! the same bytes of the session's, have found the file changed to the same bytes, not
//! empty. A file another program writes in place is truncated, then written, perhaps in
//! parts: read between them, it's empty, or part of what's written, or the old text with the
//! new over its start, any of which but the empty file may even read as a document (#479).
//! - **Empty, it's never told,** however long it stays so: it's never a document, and it's
//!   what a write in place leaves until it writes (a shell's redirect of a slow program, a
//!   copy over a slow link). Its finding is `changed` for as long as it stays, and a command
//!   still reads it as it is.
//! - **Part written, it's told only if it stays so:** read at one comparison, it's other
//!   bytes at the next, so it's never told. A writer that leaves a part of its text for
//!   longer than an interval has it read the same twice in a row, so that part is told, and
//!   reloaded as a broken file is, then the whole once it's written: two reloads. That's the
//!   limit of settling.
//!
//! Its finding is `changed` from the first comparison all the same, as the file is. A file
//! that stays as it was written, a broken one too, is told at the second comparison, an
//! interval later. Told each time a comparison finds it settled, not only when the finding
//! changes, so a reload the session couldn't make at one comparison is asked again at the
//! next.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use sha2::Digest as _;

use crate::document::json::MAX_INPUT_LEN;
use crate::file::{Unresolved, Workspace};
use crate::notify::Notify;
use crate::worker::lock;

/// The least time from one comparison's end to the next one's start (the design's
/// `DISK_CHECK_INTERVAL`): the app's idle frame asks once a second.
pub const DISK_CHECK_INTERVAL: Duration = Duration::from_millis(DISK_CHECK_MS);

/// [`DISK_CHECK_INTERVAL`] in milliseconds, as `version`'s `limits.disk_check_ms` gives it.
pub const DISK_CHECK_MS: u64 = 1000;

/// How long a comparison may read its file before it's logged as stalled (a network mount
/// that stopped answering, say): the finding before it stands while it runs.
pub const DISK_READ_STALLED: Duration = Duration::from_secs(10);

/// What a comparison found of the file at the session's path (`status`'s `disk`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disk {
    /// The bytes the session last read or wrote.
    Unchanged,
    /// Other bytes.
    Changed,
    /// No file at its path.
    Gone,
    /// Not a regular file this user may read, or past what Silico reads of a document.
    Unreadable,
}

impl Disk {
    /// The word `status` writes.
    pub fn word(self) -> &'static str {
        match self {
            Disk::Unchanged => "unchanged",
            Disk::Changed => "changed",
            Disk::Gone => "gone",
            Disk::Unreadable => "unreadable",
        }
    }
}

/// What a read of the file found: its bytes, or why there are none to compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Bytes(Vec<u8>),
    Gone,
    Unreadable,
}

/// How a watch reads its file: the session's own read, or a test's stand-in.
pub type Reader = Arc<dyn Fn(&Path) -> Found + Send + Sync>;

/// What a watch takes the time from: the system's monotonic clock, or a test's, to drive
/// the interval without waiting it out.
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What a watch tells its session each time a comparison finds the file changed and settled,
/// the comparison before it having found the same bytes (with what it read, and the count of
/// the session's reads and writes it compared against), with no lock held, so the session can
/// reload it ([`DiskWatch::when_changed`]): only if the count is still the session's
/// ([`DiskWatch::current`]), as it checks under its own lock, since a read or write of the
/// session's may come between the finding and this.
pub type Changed = Arc<dyn Fn(&[u8], u64) + Send + Sync>;

/// A session's file watched for changes on disk.
pub struct DiskWatch {
    shared: Arc<Shared>,
}

/// A save of the session's file under way ([`DiskWatch::saving`]): no comparison, and no
/// finding, until it's [`saved`](Self::saved); dropped unsaved, the save failed.
#[must_use = "a save's bracket ends when it's saved, or dropped as failed"]
pub struct Saving<'a> {
    watch: &'a DiskWatch,
}

struct Shared {
    interval: Duration,
    read: Reader,
    now: Clock,
    notify: Option<Notify>,
    /// Told of each comparison that finds the file changed and settled: the session's
    /// reload.
    changed: OnceLock<Changed>,
    /// Called between a comparison's finding and its telling `changed`, in this crate's
    /// tests only: to hold it there.
    #[cfg(test)]
    before_changed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// How many times `changed` has been told and returned, in this crate's tests only.
    #[cfg(test)]
    changed_told: std::sync::atomic::AtomicUsize,
    /// How many comparisons have ended, each with its telling done (or none to do), in this
    /// crate's tests only.
    #[cfg(test)]
    compared: std::sync::atomic::AtomicUsize,
    state: Mutex<State>,
}

struct State {
    /// The file's path, where the session last read or wrote it (a Save As moves it).
    path: PathBuf,
    /// The bytes the session last read or wrote, and its count of reads and writes.
    held: Arc<Vec<u8>>,
    counter: u64,
    /// The last comparison's finding against the session's current bytes; none before one
    /// has ended.
    finding: Option<Disk>,
    /// The digest of the bytes the last comparison found, if it found the file changed and
    /// not empty, against the session's current bytes: a change is told only once the next
    /// comparison finds bytes of this same digest (it has settled). None after any other
    /// finding, and whenever the session reads or writes, so a change is told only for two
    /// comparisons in a row against what the session holds. A digest, not the bytes: a file
    /// may be as long as Silico reads, and stay changed for as long as a transaction is open.
    unsettled: Option<[u8; 32]>,
    /// The finding the waker was last told of: what a new one is told against, whatever
    /// the session's reads and writes cleared since (so its own save, found unchanged
    /// again, tells nothing).
    told: Option<Disk>,
    /// A comparison running.
    running: bool,
    /// The session is writing its file ([`DiskWatch::saving`]): no finding until it holds
    /// what it wrote.
    saving: bool,
    /// When the last comparison ended, if one has.
    ended: Option<Instant>,
    /// When the comparison running started, and whether it has been logged as stalled.
    started: Option<(Instant, bool)>,
}

impl Saving<'_> {
    /// The save wrote `bytes`: what the session holds now, the file unchanged.
    pub fn saved(self, bytes: Arc<Vec<u8>>) {
        hold(&mut lock(&self.watch.shared.state), Some(bytes));
        std::mem::forget(self);
    }
}

impl Drop for Saving<'_> {
    /// The save failed: what the file holds now is unknown (part written, or as it was),
    /// until the next comparison.
    fn drop(&mut self) {
        hold(&mut lock(&self.watch.shared.state), None);
    }
}

/// The session now holds `bytes`, read or written, the file as it holds it; or, with none,
/// a save failed, the bytes it holds as they were and the file unknown. Either way a
/// comparison running is dropped, and comparisons go on.
fn hold(state: &mut State, bytes: Option<Arc<Vec<u8>>>) {
    state.counter = state.counter.wrapping_add(1);
    state.saving = false;
    state.unsettled = None;
    match bytes {
        Some(bytes) => {
            state.held = bytes;
            state.finding = Some(Disk::Unchanged);
            state.told = Some(Disk::Unchanged);
        }
        None => state.finding = None,
    }
}

impl DiskWatch {
    /// A watch of the file at `path`, which the session read as `held`, resolved in
    /// `workspace` as the session resolves it, compared at most once each `interval`, with
    /// `notify` told each time a finding changes (the app's waker), with no lock held.
    pub fn new(
        path: impl Into<PathBuf>,
        workspace: Option<Workspace>,
        held: Arc<Vec<u8>>,
        interval: Duration,
        notify: Option<Notify>,
    ) -> Self {
        let read: Reader = Arc::new(move |path: &Path| read(workspace.as_ref(), path));
        Self::reading(path, held, interval, notify, read)
    }

    /// [`new`](Self::new), the file read by `read` (a test's, to hold a comparison while
    /// the session goes on).
    pub fn reading(
        path: impl Into<PathBuf>,
        held: Arc<Vec<u8>>,
        interval: Duration,
        notify: Option<Notify>,
        read: Reader,
    ) -> Self {
        Self::clocked(path, held, interval, notify, read, Arc::new(Instant::now))
    }

    /// [`reading`](Self::reading), the time taken from `now` (a test's, to say when each
    /// comparison is due without waiting for it).
    pub fn clocked(
        path: impl Into<PathBuf>,
        held: Arc<Vec<u8>>,
        interval: Duration,
        notify: Option<Notify>,
        read: Reader,
        now: Clock,
    ) -> Self {
        DiskWatch {
            shared: Arc::new(Shared {
                interval,
                read,
                now,
                notify,
                changed: OnceLock::new(),
                #[cfg(test)]
                before_changed: Mutex::new(None),
                #[cfg(test)]
                changed_told: std::sync::atomic::AtomicUsize::new(0),
                #[cfg(test)]
                compared: std::sync::atomic::AtomicUsize::new(0),
                state: Mutex::new(State {
                    path: path.into(),
                    held,
                    counter: 0,
                    // None before a first comparison ends (`status`'s `null`).
                    finding: None,
                    unsettled: None,
                    told: None,
                    running: false,
                    saving: false,
                    ended: None,
                    started: None,
                }),
            }),
        }
    }

    /// From now on, `changed` is told of each comparison that finds the file changed to the
    /// bytes the comparison before it found, not empty (settled), with those bytes, with no
    /// lock held: the session's reload of a file changed on disk (A3). Given once, as the
    /// session is made; given again, it's ignored.
    pub fn when_changed(&self, changed: Changed) {
        let _ = self.shared.changed.set(changed);
    }

    /// Whether `counter`, a count of the session's reads and writes a comparison was made
    /// against (as [`Changed`] is told it), is still the session's: no read or write since.
    pub fn current(&self, counter: u64) -> bool {
        lock(&self.shared.state).counter == counter
    }

    /// From now on, each comparison that finds the file changed and settled calls `hold`
    /// after its finding is kept and before it tells the session, in this crate's tests
    /// only.
    #[cfg(test)]
    pub(crate) fn before_changed(&self, hold: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.shared.before_changed) = Some(hold);
    }

    /// How many times a comparison has told `changed` and it returned, in this crate's tests
    /// only.
    #[cfg(test)]
    pub(crate) fn changed_told(&self) -> usize {
        self.shared
            .changed_told
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many comparisons have ended, each with its finding kept or dropped and its
    /// telling done, in this crate's tests only: once it has moved, a comparison that told
    /// nothing never will.
    #[cfg(test)]
    pub(crate) fn compared(&self) -> usize {
        self.shared
            .compared
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The session read `bytes` (a reload): what it holds now. A comparison running is
    /// against the bytes before, and its finding is dropped; the file is as the session
    /// holds it, unchanged, until a comparison says otherwise.
    pub fn held(&self, bytes: Arc<Vec<u8>>) {
        hold(&mut lock(&self.shared.state), Some(bytes));
    }

    /// The session saved its document as `bytes` at `path`, its file from now on (a Save
    /// As): what it holds now, compared there. A comparison running is of the file before,
    /// and its finding is dropped.
    pub fn moved(&self, path: impl Into<PathBuf>, bytes: Arc<Vec<u8>>) {
        let mut state = lock(&self.shared.state);
        state.path = path.into();
        hold(&mut state, Some(bytes));
    }

    /// The session is about to write its file: until the [`Saving`] is
    /// [`saved`](Saving::saved) with what it wrote, or dropped (the save failed), no
    /// comparison starts, one running is dropped, and there's no finding. So a comparison
    /// that reads the file mid-save, or the saved file before the session holds it, never
    /// reads the session's own save as a change.
    pub fn saving(&self) -> Saving<'_> {
        let mut state = lock(&self.shared.state);
        state.counter = state.counter.wrapping_add(1);
        state.finding = None;
        state.saving = true;
        Saving { watch: self }
    }

    /// The last comparison's finding against what the session holds now, none before one
    /// has ended; and the next comparison started, on a thread of its own, if none is
    /// running and the last ended at least the watch's interval ago. Never waits on the
    /// file.
    pub fn finding(&self) -> Option<Disk> {
        let now = (self.shared.now)();
        let mut guard = lock(&self.shared.state);
        // Borrowed whole, so its fields borrow apart.
        let state = &mut *guard;
        let due = state
            .ended
            .is_none_or(|ended| now.saturating_duration_since(ended) >= self.shared.interval);
        if let Some((started, logged)) = &mut state.started
            && !*logged
            && now.saturating_duration_since(*started) >= DISK_READ_STALLED
        {
            *logged = true;
            tracing::warn!(
                path = %crate::file::shown_path(&state.path),
                "the file's comparison on disk has read it for {} s and still hasn't ended: its last finding stands",
                DISK_READ_STALLED.as_secs()
            );
        }
        if !state.running && !state.saving && due {
            state.running = true;
            state.started = Some((now, false));
            let counter = state.counter;
            let held = Arc::clone(&state.held);
            let path = state.path.clone();
            let shared = Arc::clone(&self.shared);
            let spawned = std::thread::Builder::new()
                .name("silico-disk".to_owned())
                .spawn(move || compare(&shared, &path, counter, &held));
            if let Err(error) = spawned {
                // No thread to spare: the next asks again after the interval, the last
                // finding standing.
                tracing::warn!("couldn't start a disk comparison: {error}");
                state.running = false;
                state.ended = Some(now);
            }
        }
        state.finding
    }
}

/// One comparison of the file at `path` with `held`, the session's bytes at `counter`:
/// its finding kept if the session still holds them, and told if it changed and settled.
fn compare(shared: &Shared, path: &Path, counter: u64, held: &[u8]) {
    compare_once(shared, path, counter, held);
    #[cfg(test)]
    shared
        .compared
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// [`compare`]'s work.
fn compare_once(shared: &Shared, path: &Path, counter: u64, held: &[u8]) {
    let _span = tracing::debug_span!("disk_compare").entered();
    // If the read panics (a test's reader), the comparison has ended all the same: the
    // next can start.
    let ended = Ended(shared);
    let (found, bytes) = match (shared.read)(path) {
        Found::Bytes(bytes) if bytes.as_slice() == held => (Disk::Unchanged, None),
        Found::Bytes(bytes) => (Disk::Changed, Some(bytes)),
        Found::Gone => (Disk::Gone, None),
        Found::Unreadable => (Disk::Unreadable, None),
    };
    // What the file changed to, as the next comparison must find it for the change to have
    // settled: worked out before the lock is taken, which `status` takes each frame. None
    // for an empty file, which never settles: it's never a document, and it's what a write
    // in place leaves until it writes, however long that takes (#479).
    let changed_to = bytes
        .as_deref()
        .filter(|bytes| !bytes.is_empty())
        .map(digest);
    let now = (shared.now)();
    let mut state = lock(&shared.state);
    // Ended in the step its finding is kept in, so no one who sees the finding sees the
    // comparison running, or its end stamped after.
    std::mem::forget(ended);
    state.running = false;
    state.ended = Some(now);
    state.started = None;
    // The session read or wrote since: this was against its bytes before.
    if state.counter != counter {
        return;
    }
    // Settled: the comparison before this one, against the same bytes of the session's,
    // found the file changed to these same bytes. Read mid-write (truncated, part written,
    // still growing), it's other bytes at the next comparison, so it's never told (#479).
    let settled = changed_to.is_some() && state.unsettled == changed_to;
    state.unsettled = changed_to;
    let told = state.told != Some(found);
    state.finding = Some(found);
    state.told = Some(found);
    drop(state);
    if told && let Some(notify) = &shared.notify {
        notify();
    }
    if settled
        && let Some(bytes) = &bytes
        && let Some(changed) = shared.changed.get()
    {
        #[cfg(test)]
        {
            let hold = lock(&shared.before_changed).clone();
            if let Some(hold) = hold {
                hold();
            }
        }
        // Told with the count it compared against: the session may have read or written
        // since the finding was kept, the lock let go (its own reload, taking these bytes).
        changed(bytes, counter);
        #[cfg(test)]
        shared
            .changed_told
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The SHA-256 digest of a file's bytes: what the disk watch keeps of a change until it
/// settles, what the session's own reload checks the file still holds before it loads it,
/// and what a reload the session refused keeps, so the same bytes aren't queued again.
pub(crate) fn digest(bytes: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(bytes).into()
}

/// A comparison's end if its read panics: none running, and the time it ended.
struct Ended<'a>(&'a Shared);

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        let now = (self.0.now)();
        let mut state = lock(&self.0.state);
        state.running = false;
        state.ended = Some(now);
        state.started = None;
    }
}

/// The file at `path` as the session reads a document ([`crate::file::read`]), resolved in
/// `workspace` first, as the session resolves it to reload: its bytes; gone if nothing is
/// at its path; unreadable if it leads out of the workspace, isn't a regular file this user
/// may read, or is longer than Silico reads of one. One open decides: a file removed and
/// made again around it is one or the other, never misread.
fn read(workspace: Option<&Workspace>, path: &Path) -> Found {
    let resolved = match crate::file::resolve(workspace, path) {
        Ok(resolved) => resolved,
        Err(Unresolved::Unreachable(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Found::Gone;
        }
        Err(_) => return Found::Unreadable,
    };
    match crate::file::opened_and_read_io(&resolved) {
        Ok((_, bytes)) if bytes.len() > MAX_INPUT_LEN => Found::Unreadable,
        Ok((_, bytes)) => Found::Bytes(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Found::Gone,
        Err(_) => Found::Unreadable,
    }
}
