//! Hostile inputs for `silico_core::disk` (break-it, PR #266): what is at the session's path
//! (links, pipes, devices, sockets, a file past the limit, a permission removed, a file
//! swapped as it's read), and when (the session's own save racing a comparison, a waker
//! that calls back in, a reader that panics, a watch dropped mid-comparison).

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use silico_core::disk::{Disk, DiskWatch, Found, Reader};

const WAIT: Duration = Duration::from_secs(10);

fn settled(watch: &DiskWatch) -> Disk {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(found) = watch.finding() {
            return found;
        }
        assert!(Instant::now() < deadline, "no comparison ended");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn watch(path: &Path, held: &[u8]) -> DiskWatch {
    DiskWatch::new(path, Arc::new(held.to_vec()), Duration::ZERO, None)
}

/// Asked until a comparison has ended with a finding other than `not`, within [`WAIT`].
fn settled_other(watch: &DiskWatch, not: Disk) -> Disk {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(found) = watch.finding()
            && found != not
        {
            return found;
        }
        assert!(Instant::now() < deadline, "still {not:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ---- What is at the path ----

#[test]
fn a_symlink_to_the_file_is_followed_and_compared_by_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.silico");
    std::fs::write(&file, b"A").unwrap();
    let link = dir.path().join("link.silico");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert_eq!(settled(&watch(&link, b"A")), Disk::Unchanged);
    assert_eq!(settled(&watch(&link, b"B")), Disk::Changed);
}

#[test]
fn a_dangling_symlink_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("dangling.silico");
    std::os::unix::fs::symlink(dir.path().join("nothing"), &link).unwrap();
    assert_eq!(settled(&watch(&link, b"A")), Disk::Gone);
}

#[test]
fn a_symlink_to_a_directory_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let link = dir.path().join("dir.silico");
    std::os::unix::fs::symlink(&sub, &link).unwrap();
    assert_eq!(settled(&watch(&link, b"A")), Disk::Unreadable);
}

#[test]
fn a_symlink_loop_is_unreadable_not_a_hang() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.silico");
    let b = dir.path().join("b.silico");
    std::os::unix::fs::symlink(&b, &a).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();
    assert_eq!(settled(&watch(&a, b"A")), Disk::Unreadable);
}

#[test]
fn character_devices_are_unreadable_without_reading_them() {
    // /dev/zero never ends and /dev/null is empty: neither is a document, and an empty held
    // document must not read as "unchanged" against /dev/null.
    for device in ["/dev/zero", "/dev/null", "/dev/random"] {
        let path = Path::new(device);
        assert_eq!(settled(&watch(path, b"")), Disk::Unreadable, "{device}");
    }
}

#[test]
fn a_unix_socket_at_the_path_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.silico");
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    assert_eq!(settled(&watch(&path, b"A")), Disk::Unreadable);
}

#[test]
fn a_fifo_with_a_writer_holding_it_open_is_unreadable_and_never_read() {
    // A writer that keeps the pipe open: an open (or read) that went ahead would read what
    // it writes, or wait on it forever.
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("fifo.silico");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    // Opened read-write, so the open doesn't wait for a reader: a writer the pipe has.
    let writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)
        .unwrap();
    assert_eq!(settled(&watch(&fifo, b"A")), Disk::Unreadable);
    drop(writer);
}

#[test]
fn an_empty_file_held_empty_is_unchanged_and_the_empty_path_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.silico");
    std::fs::write(&path, b"").unwrap();
    assert_eq!(settled(&watch(&path, b"")), Disk::Unchanged);
    assert_eq!(settled(&watch(&path, b"x")), Disk::Changed);
    assert_eq!(settled(&watch(Path::new(""), b"")), Disk::Gone);
}

#[test]
fn a_path_with_a_nul_byte_is_unreadable_not_a_panic() {
    use std::os::unix::ffi::OsStrExt as _;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/a\0b.silico"));
    assert_eq!(settled(&watch(path, b"A")), Disk::Unreadable);
}

#[test]
fn a_file_at_exactly_the_limit_is_compared_and_one_byte_past_it_is_unreadable() {
    use silico_core::document::json::MAX_INPUT_LEN;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.silico");
    let bytes = vec![b' '; MAX_INPUT_LEN];
    std::fs::write(&path, &bytes).unwrap();
    let held = Arc::new(bytes);
    let w = DiskWatch::new(&path, Arc::clone(&held), Duration::ZERO, None);
    assert_eq!(settled(&w), Disk::Unchanged);
    // The file grows by one byte past the limit: unreadable, never "changed" by a
    // truncated compare (the first MAX_INPUT_LEN bytes still match the held ones).
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut file, b" ").unwrap();
    drop(file);
    assert_eq!(settled_other(&w, Disk::Unchanged), Disk::Unreadable);
}

#[test]
fn a_huge_sparse_file_is_unreadable_and_read_only_to_the_limit() {
    // A terabyte (sparse, so free to make): the comparison reads no more than the limit and
    // ends promptly, each time.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.silico");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(1 << 40).unwrap();
    drop(file);
    let w = watch(&path, b"A");
    let started = Instant::now();
    assert_eq!(settled(&w), Disk::Unreadable);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_permission_removed_is_unreadable_and_restored_is_unchanged_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.silico");
    std::fs::write(&path, b"A").unwrap();
    let w = watch(&path, b"A");
    assert_eq!(settled(&w), Disk::Unchanged);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(settled_other(&w, Disk::Unchanged), Disk::Unreadable);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(settled_other(&w, Disk::Unreadable), Disk::Unchanged);
}

#[test]
fn a_directory_permission_removed_is_unreadable_not_gone() {
    // The file is there; only the way to it is shut. Gone would tell the session the file
    // was deleted.
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let path = sub.join("a.silico");
    std::fs::write(&path, b"A").unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000)).unwrap();
    let found = settled(&watch(&path, b"A"));
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(found, Disk::Unreadable);
}

#[test]
fn a_parent_replaced_by_a_file_is_neither_unchanged_nor_changed() {
    // `dir/a.silico` where `dir` is now a regular file (ENOTDIR). Spec question: gone or
    // unreadable. Never a comparison of bytes.
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("parent");
    std::fs::write(&parent, b"A").unwrap();
    let found = settled(&watch(&parent.join("a.silico"), b"A"));
    assert!(matches!(found, Disk::Gone | Disk::Unreadable), "{found:?}");
}

#[test]
fn a_file_replaced_by_a_directory_then_a_file_again_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.silico");
    std::fs::write(&path, b"A").unwrap();
    let w = watch(&path, b"A");
    assert_eq!(settled(&w), Disk::Unchanged);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(settled_other(&w, Disk::Unchanged), Disk::Unreadable);
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(settled_other(&w, Disk::Unreadable), Disk::Gone);
    std::fs::write(&path, b"A").unwrap();
    assert_eq!(settled_other(&w, Disk::Gone), Disk::Unchanged);
}

#[test]
fn a_file_deleted_and_made_again_reads_only_as_gone_or_its_bytes() {
    // Found by break-it, fixed (a failed read looks again, and gone is gone): `read` stats the path, then `file::read` opens it. A file deleted between the
    // two reads as `Unreadable` (file::read's open fails with ENOENT, and every error of
    // file::read is unreadable), not `Gone`: an editor that saves by unlink-and-create, or
    // a `git checkout` that removes and writes the file, flickers the session's status to
    // "unreadable", and the waker fires for it. Raced, so it's tried for a few seconds.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.silico");
    std::fs::write(&path, b"A").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let churn = {
        let (stop, path) = (Arc::clone(&stop), path.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::write(&path, b"A");
            }
        })
    };
    let w = watch(&path, b"A");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = std::collections::BTreeMap::new();
    while Instant::now() < deadline {
        if let Some(found) = w.finding() {
            *seen.entry(found.word()).or_insert(0u64) += 1;
            if found == Disk::Unreadable {
                break;
            }
        }
        std::thread::yield_now();
    }
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
    // A file whose only states are absent and "A" is gone or unchanged; anything else
    // misreports it. (Changed is the file truncated, empty, before "A" is written: true at
    // that moment.)
    assert!(
        !seen.contains_key("unreadable"),
        "a file deleted between the stat and the open read as unreadable: {seen:?}"
    );
}

// ---- When: the session's own save against a comparison ----

/// A disk the stand-in reader answers from, and a gate each read waits on.
struct Disk2 {
    bytes: Arc<Mutex<Vec<u8>>>,
    reads: Arc<AtomicUsize>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl Disk2 {
    fn new(bytes: &[u8]) -> Self {
        Disk2 {
            bytes: Arc::new(Mutex::new(bytes.to_vec())),
            reads: Arc::default(),
            gate: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    /// Reads the disk after the gate opens (so the read sees what's on disk then).
    fn reader(&self) -> Reader {
        let (bytes, reads, gate) = (
            Arc::clone(&self.bytes),
            Arc::clone(&self.reads),
            Arc::clone(&self.gate),
        );
        Arc::new(move |_| {
            reads.fetch_add(1, Ordering::SeqCst);
            let mut open = gate.0.lock().unwrap();
            while !*open {
                open = gate.1.wait(open).unwrap();
            }
            drop(open);
            Found::Bytes(bytes.lock().unwrap().clone())
        })
    }

    fn write(&self, bytes: &[u8]) {
        *self.bytes.lock().unwrap() = bytes.to_vec();
    }

    fn open(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    fn wait_reads(&self, n: usize) {
        let deadline = Instant::now() + WAIT;
        while self.reads.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// How many times the waker was told, and the waker.
fn told() -> (Arc<AtomicUsize>, silico_core::drag::Notify) {
    let told = Arc::new(AtomicUsize::new(0));
    let t = Arc::clone(&told);
    (
        told,
        Arc::new(move || {
            t.fetch_add(1, Ordering::SeqCst);
        }),
    )
}

#[test]
fn the_sessions_own_save_written_then_held_never_reads_as_changed() {
    // Found by break-it, fixed (a save is bracketed: `saving` before it writes, `held`
    // after): the module promises "the session's own save never reads as a change on
    // disk". The session saves B (the file is B on disk), then tells the watch it holds B.
    // A comparison begun before the save (against A, at the old counter) reads the file
    // after the save, and ends before `held(B)`: its counter is still the session's, so
    // it's kept as `Changed`, the waker told, and `status` says the file changed on disk
    // for the session's own save. Nothing in the API lets a session close the window: no
    // call before the write that holds findings until `held` after it.
    let disk = Disk2::new(b"A");
    let (told, notify) = told();
    let w = DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::from_secs(3600),
        Some(notify),
        disk.reader(),
    );
    assert_eq!(w.finding(), None);
    disk.wait_reads(1);
    // The session's save, bracketed as a save is: the file is B.
    w.saving();
    disk.write(b"B");
    // The comparison reads it and ends, before the session says it holds B.
    disk.open();
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut seen = None;
    while Instant::now() < deadline {
        if let Some(found) = w.finding() {
            seen = Some(found);
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    w.held(Arc::new(b"B".to_vec()));
    assert_ne!(
        seen,
        Some(Disk::Changed),
        "the session's own save read as changed"
    );
    assert_eq!(
        told.load(Ordering::SeqCst),
        0,
        "the waker told of its own save"
    );
}

#[test]
fn the_sessions_own_save_held_then_written_never_reads_as_changed() {
    // Found by break-it, fixed (as above): the other order. The session says it holds B, then writes B (a save's temporary
    // file, fsync, and rename take time). A comparison begun after `held(B)` (the current
    // counter) reads the file before the rename lands, still A: `Changed`, kept, told.
    let disk = Disk2::new(b"A");
    let (told, notify) = told();
    let w = DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::ZERO,
        Some(notify),
        disk.reader(),
    );
    disk.open();
    assert_eq!(settled(&w), Disk::Unchanged);
    let before = told.load(Ordering::SeqCst);
    // The save, bracketed: while it writes, nothing is compared or found; then it holds B.
    w.saving();
    for _ in 0..20 {
        assert_eq!(w.finding(), None, "a finding mid-save");
    }
    disk.write(b"B");
    w.held(Arc::new(b"B".to_vec()));
    let found = settled(&w);
    assert_eq!(found, Disk::Unchanged);
    assert_ne!(
        found,
        Disk::Changed,
        "the session's own save read as changed"
    );
    assert_eq!(
        told.load(Ordering::SeqCst),
        before,
        "the waker told of its own save"
    );
}

#[test]
fn findings_are_always_against_what_the_session_holds_now() {
    // Many threads asking while the session's bytes flip between the file's and others:
    // under one outer lock with each `held`, every finding asked must be the one its
    // current bytes give (the file never changes). A stale finding (against bytes before)
    // would show as unchanged while holding other bytes, or changed while holding the file's.
    let file = b"FILE".to_vec();
    let reader: Reader = {
        let file = file.clone();
        Arc::new(move |_| {
            std::thread::yield_now();
            Found::Bytes(file.clone())
        })
    };
    let w = Arc::new(DiskWatch::reading(
        "p",
        Arc::new(file.clone()),
        Duration::ZERO,
        None,
        reader,
    ));
    // The session's current bytes, and the lock under which it changes them and asks.
    let current = Arc::new(Mutex::new(true));
    let stop = Arc::new(AtomicBool::new(false));
    let checked = Arc::new(AtomicUsize::new(0));
    let askers: Vec<_> = (0..4)
        .map(|_| {
            let (w, current, stop, checked) = (
                Arc::clone(&w),
                Arc::clone(&current),
                Arc::clone(&stop),
                Arc::clone(&checked),
            );
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let holds_file = current.lock().unwrap();
                    if let Some(found) = w.finding() {
                        let expected = if *holds_file {
                            Disk::Unchanged
                        } else {
                            Disk::Changed
                        };
                        assert_eq!(found, expected, "a finding against bytes before");
                        checked.fetch_add(1, Ordering::Relaxed);
                    }
                    drop(holds_file);
                    std::thread::yield_now();
                }
            })
        })
        .collect();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..20_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let holds_file = seed & 1 == 0;
        let mut current = current.lock().unwrap();
        *current = holds_file;
        w.held(Arc::new(if holds_file {
            file.clone()
        } else {
            b"OTHER".to_vec()
        }));
        drop(current);
        if seed & 6 == 0 {
            std::thread::sleep(Duration::from_micros(50));
        }
    }
    stop.store(true, Ordering::Relaxed);
    for asker in askers {
        asker.join().unwrap();
    }
    assert!(
        checked.load(Ordering::Relaxed) > 100,
        "too few findings checked"
    );
}

// ---- When: the waker, the reader, the watch's life ----

#[test]
fn a_waker_that_calls_back_into_the_watch_neither_deadlocks_nor_loops() {
    // The app's waker might ask for the finding at once, or (a reload on a change) say the
    // session holds new bytes. Neither may deadlock (the waker runs with no lock held), and
    // a waker that asks again must not start comparisons without end when nothing changes.
    let reads = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&reads);
    let reader: Reader = Arc::new(move |_| {
        r.fetch_add(1, Ordering::SeqCst);
        Found::Bytes(b"A".to_vec())
    });
    let cell: Arc<Mutex<Option<Arc<DiskWatch>>>> = Arc::default();
    let told = Arc::new(AtomicUsize::new(0));
    let notify = {
        let (cell, told) = (Arc::clone(&cell), Arc::clone(&told));
        Arc::new(move || {
            let n = told.fetch_add(1, Ordering::SeqCst);
            let w = cell.lock().unwrap().clone();
            if let Some(w) = w {
                let _ = w.finding();
                if n == 0 {
                    // A reload of other bytes: the next comparison finds them changed.
                    w.held(Arc::new(b"B".to_vec()));
                    let _ = w.finding();
                }
            }
        })
    };
    let w = Arc::new(DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::ZERO,
        Some(notify),
        reader,
    ));
    *cell.lock().unwrap() = Some(Arc::clone(&w));
    assert_eq!(settled_other(&w, Disk::Unchanged), Disk::Changed);
    std::thread::sleep(Duration::from_millis(100));
    // Told once for unchanged and once for changed; the waker's own asks at a zero interval
    // start comparisons, but with nothing changing, they tell no one.
    assert_eq!(told.load(Ordering::SeqCst), 2);
    *cell.lock().unwrap() = None;
}

#[test]
fn a_reader_that_panics_once_does_not_stop_every_comparison_after() {
    // Found by break-it, fixed (the comparison's end is a guard): `compare` set `running` false only after the read returns. A read that
    // panics (a test's reader; any reader the `Reader` type admits) kills the comparison's
    // thread with `running` still true: no comparison ever starts again, and `finding`
    // answers its last finding (or none) for the rest of the session.
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let reader: Reader = Arc::new(move |_| {
        if c.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("a read that fails hard, once");
        }
        Found::Bytes(b"A".to_vec())
    });
    let w = DiskWatch::reading("p", Arc::new(b"A".to_vec()), Duration::ZERO, None, reader);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut found = None;
    while Instant::now() < deadline && found.is_none() {
        found = w.finding();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        found,
        Some(Disk::Unchanged),
        "{} reads",
        calls.load(Ordering::SeqCst)
    );
}

#[test]
fn a_watch_dropped_mid_comparison_lets_its_thread_end() {
    // The comparison's thread holds the watch's state; dropped mid-read, it must still end
    // (no hang, no panic) and release it.
    let disk = Disk2::new(b"A");
    let (_told, notify) = told();
    let w = DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::ZERO,
        Some(notify),
        disk.reader(),
    );
    assert_eq!(w.finding(), None);
    disk.wait_reads(1);
    drop(w);
    disk.open();
    // The thread ends, and its reader (holding the disk's bytes) with it.
    let deadline = Instant::now() + WAIT;
    while Arc::strong_count(&disk.bytes) > 1 {
        assert!(
            Instant::now() < deadline,
            "the comparison's thread never ended"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn many_watches_each_held_mid_read_run_one_thread_each_and_never_wait() {
    // Hung reads (a stalled network share): each watch has one comparison running, its
    // `finding` answering at once, however often asked; no more threads start.
    let disk = Disk2::new(b"A");
    let watches: Vec<_> = (0..64)
        .map(|_| {
            DiskWatch::reading(
                "p",
                Arc::new(b"A".to_vec()),
                Duration::ZERO,
                None,
                disk.reader(),
            )
        })
        .collect();
    let started = Instant::now();
    for _ in 0..100 {
        for w in &watches {
            assert_eq!(w.finding(), None);
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    disk.wait_reads(64);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(disk.reads.load(Ordering::SeqCst), 64);
    disk.open();
    for w in &watches {
        assert_eq!(settled(w), Disk::Unchanged);
    }
}

#[test]
fn an_interval_of_duration_max_compares_once_and_never_overflows() {
    let reads = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&reads);
    let reader: Reader = Arc::new(move |_| {
        r.fetch_add(1, Ordering::SeqCst);
        Found::Gone
    });
    let w = DiskWatch::reading("p", Arc::new(Vec::new()), Duration::MAX, None, reader);
    assert_eq!(settled(&w), Disk::Gone);
    for _ in 0..1000 {
        assert_eq!(w.finding(), Some(Disk::Gone));
    }
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[test]
fn a_held_with_no_comparison_running_clears_the_finding_until_the_next_ends() {
    // After a reload with nothing running, the finding is none (not the old one against
    // other bytes), and the next comparison is against what's held now.
    let answer = Arc::new(Mutex::new(Found::Bytes(b"A".to_vec())));
    let a = Arc::clone(&answer);
    let reader: Reader = Arc::new(move |_| a.lock().unwrap().clone());
    let w = DiskWatch::reading("p", Arc::new(b"A".to_vec()), Duration::ZERO, None, reader);
    assert_eq!(settled(&w), Disk::Unchanged);
    std::thread::sleep(Duration::from_millis(20));
    w.held(Arc::new(b"B".to_vec()));
    assert_eq!(settled(&w), Disk::Changed);
    *answer.lock().unwrap() = Found::Bytes(b"B".to_vec());
    assert_eq!(settled_other(&w, Disk::Changed), Disk::Unchanged);
}
