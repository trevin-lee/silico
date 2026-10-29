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
//! finding stands until the read ends.
//!
//! It reloads nothing: what to do with a finding is the session's (A3).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::document::json::MAX_INPUT_LEN;
use crate::file::{Unresolved, Workspace};
use crate::notify::Notify;
use crate::worker::lock;

/// The least time from one comparison's end to the next one's start (the design's
/// `DISK_CHECK_INTERVAL`): the app's idle frame asks once a second.
pub const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(1);

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
    path: PathBuf,
    interval: Duration,
    read: Reader,
    now: Clock,
    notify: Option<Notify>,
    state: Mutex<State>,
}

struct State {
    /// The bytes the session last read or wrote, and its count of reads and writes.
    held: Arc<Vec<u8>>,
    counter: u64,
    /// The last comparison's finding against the session's current bytes; none before one
    /// has ended.
    finding: Option<Disk>,
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
                path: path.into(),
                interval,
                read,
                now,
                notify,
                state: Mutex::new(State {
                    held,
                    counter: 0,
                    // None before a first comparison ends (`status`'s `null`).
                    finding: None,
                    told: None,
                    running: false,
                    saving: false,
                    ended: None,
                }),
            }),
        }
    }

    /// The session read `bytes` (a reload): what it holds now. A comparison running is
    /// against the bytes before, and its finding is dropped; the file is as the session
    /// holds it, unchanged, until a comparison says otherwise.
    pub fn held(&self, bytes: Arc<Vec<u8>>) {
        hold(&mut lock(&self.shared.state), Some(bytes));
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
        let mut state = lock(&self.shared.state);
        let due = state
            .ended
            .is_none_or(|ended| now.saturating_duration_since(ended) >= self.shared.interval);
        if !state.running && !state.saving && due {
            state.running = true;
            let counter = state.counter;
            let held = Arc::clone(&state.held);
            let shared = Arc::clone(&self.shared);
            let spawned = std::thread::Builder::new()
                .name("silico-disk".to_owned())
                .spawn(move || compare(&shared, counter, &held));
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

/// One comparison of `shared`'s file with `held`, the session's bytes at `counter`: its
/// finding kept if the session still holds them, and told if it changed.
fn compare(shared: &Shared, counter: u64, held: &[u8]) {
    let _span = tracing::debug_span!("disk_compare").entered();
    // However the read ends, a panic too (a test's reader), the comparison has: the next
    // can start.
    let _ended = Ended(shared);
    let found = match (shared.read)(&shared.path) {
        Found::Bytes(bytes) if bytes == held => Disk::Unchanged,
        Found::Bytes(_) => Disk::Changed,
        Found::Gone => Disk::Gone,
        Found::Unreadable => Disk::Unreadable,
    };
    let mut state = lock(&shared.state);
    // The session read or wrote since: this was against its bytes before.
    if state.counter != counter {
        return;
    }
    let changed = state.told != Some(found);
    state.finding = Some(found);
    state.told = Some(found);
    drop(state);
    if changed && let Some(notify) = &shared.notify {
        notify();
    }
}

/// A comparison's end, however it ends: none running, and the time it ended.
struct Ended<'a>(&'a Shared);

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        let now = (self.0.now)();
        let mut state = lock(&self.0.state);
        state.running = false;
        state.ended = Some(now);
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
