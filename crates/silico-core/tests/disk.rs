//! `silico_core::disk` (docs/design/m5.0-app.md, section 4, *A file changed on disk*): a
//! session's file compared off the caller's thread, by bytes, at most one comparison at a
//! time and one each interval, a finding against bytes the session no longer holds
//! dropped. Real files for what a comparison finds; a held stand-in reader for when.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use silico_core::disk::{Disk, DiskWatch, Found, Reader};

/// How long a test waits for something it expects.
const WAIT: Duration = Duration::from_secs(10);

/// `watch`'s first finding, asked for until one has ended, within [`WAIT`].
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

fn watch(path: &std::path::Path, held: &[u8]) -> DiskWatch {
    DiskWatch::new(path, Arc::new(held.to_vec()), Duration::ZERO, None)
}

#[test]
fn a_file_as_the_session_holds_it_is_unchanged_and_other_bytes_are_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.silico");
    std::fs::write(&path, b"{\"silico\": 1}").unwrap();
    let w = watch(&path, b"{\"silico\": 1}");
    assert_eq!(settled(&w), Disk::Unchanged);
    // A byte more: changed, though its time and length might say nothing.
    std::fs::write(&path, b"{\"silico\": 1} ").unwrap();
    let w = watch(&path, b"{\"silico\": 1}");
    assert_eq!(settled(&w), Disk::Changed);
}

#[test]
fn no_file_is_gone_and_a_directory_or_a_pipe_is_unreadable_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        settled(&watch(&dir.path().join("none.silico"), b"x")),
        Disk::Gone
    );
    let sub = dir.path().join("sub.silico");
    std::fs::create_dir(&sub).unwrap();
    assert_eq!(settled(&watch(&sub, b"x")), Disk::Unreadable);
    // A pipe with no writer: an open that waited would hang the comparison.
    let fifo = dir.path().join("fifo.silico");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(settled(&watch(&fifo, b"x")), Disk::Unreadable);
}

/// A reader that counts its reads and holds each until the gate opens, answering
/// `answer`.
struct Held {
    reads: Arc<AtomicUsize>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl Held {
    fn new() -> Self {
        Held {
            reads: Arc::default(),
            gate: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn reader(&self, answer: Found) -> Reader {
        let (reads, gate) = (Arc::clone(&self.reads), Arc::clone(&self.gate));
        Arc::new(move |_| {
            reads.fetch_add(1, Ordering::SeqCst);
            let mut open = gate.0.lock().unwrap();
            while !*open {
                open = gate.1.wait(open).unwrap();
            }
            answer.clone()
        })
    }

    fn open(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn wait_reads(&self, n: usize) {
        let deadline = Instant::now() + WAIT;
        while self.reads() < n {
            assert!(Instant::now() < deadline, "{} reads, not {n}", self.reads());
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn a_finding_never_waits_on_the_file_and_one_comparison_runs_at_a_time() {
    let held = Held::new();
    let w = DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::ZERO,
        None,
        held.reader(Found::Bytes(b"A".to_vec())),
    );
    // Asked again and again while the first is held: none ended, none more started.
    for _ in 0..50 {
        assert_eq!(w.finding(), None);
    }
    held.wait_reads(1);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(held.reads(), 1);
    held.open();
    assert_eq!(settled(&w), Disk::Unchanged);
}

#[test]
fn a_comparison_against_bytes_the_session_no_longer_holds_is_dropped() {
    // The session holds A; a comparison reads the file, which its own save has made B, while
    // the session moves on to hold B. The comparison was against A: dropped, never
    // reported as changed. Then A again (a save of A after B): a comparison begun at A
    // would match it by bytes, but it's dropped all the same, by the counter.
    for (now, file) in [(b"B", b"B"), (b"A", b"A")] {
        let held = Held::new();
        let w = DiskWatch::reading(
            "p",
            Arc::new(b"A".to_vec()),
            Duration::from_secs(3600),
            None,
            held.reader(Found::Bytes(file.to_vec())),
        );
        assert_eq!(w.finding(), None);
        held.wait_reads(1);
        w.held(Arc::new(b"B".to_vec()));
        if now == b"A" {
            w.held(Arc::new(b"A".to_vec()));
        }
        held.open();
        // Ended, and its finding dropped; the hour's interval starts no other.
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            assert_eq!(w.finding(), None, "a finding against the bytes before");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(held.reads(), 1);
    }
}

/// A reader answering A, and how many times it's read.
fn counting() -> (Reader, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reads);
    let reader: Reader = Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Found::Bytes(b"A".to_vec())
    });
    (reader, reads)
}

#[test]
fn the_next_comparison_waits_its_interval_from_the_last_ones_end() {
    // An hour's interval: answered from the last, read no more however often it's asked.
    let (reader, reads) = counting();
    let w = DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::from_secs(3600),
        None,
        reader,
    );
    assert_eq!(settled(&w), Disk::Unchanged);
    for _ in 0..100 {
        assert_eq!(w.finding(), Some(Disk::Unchanged));
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    // A short one: asked past it, read again.
    let (reader, reads) = counting();
    let w = DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::from_millis(20),
        None,
        reader,
    );
    assert_eq!(settled(&w), Disk::Unchanged);
    let deadline = Instant::now() + WAIT;
    while reads.load(Ordering::SeqCst) < 2 {
        w.finding();
        assert!(Instant::now() < deadline, "no second comparison");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_waker_is_told_when_a_finding_changes_and_not_when_it_stays() {
    let told = Arc::new(AtomicUsize::new(0));
    let t = Arc::clone(&told);
    let answer = Arc::new(Mutex::new(Found::Bytes(b"A".to_vec())));
    let a = Arc::clone(&answer);
    let reads = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&reads);
    let reader: Reader = Arc::new(move |_| {
        r.fetch_add(1, Ordering::SeqCst);
        a.lock().unwrap().clone()
    });
    let w = DiskWatch::reading(
        "p",
        Arc::new(b"A".to_vec()),
        Duration::ZERO,
        Some(Arc::new(move || {
            t.fetch_add(1, Ordering::SeqCst);
        })),
        reader,
    );
    let after = |n: usize| {
        let deadline = Instant::now() + WAIT;
        while reads.load(Ordering::SeqCst) < n {
            w.finding();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        // That comparison's end, too.
        std::thread::sleep(Duration::from_millis(20));
        w.finding()
    };
    assert_eq!(after(3), Some(Disk::Unchanged));
    assert_eq!(
        told.load(Ordering::SeqCst),
        1,
        "told once: none to unchanged"
    );
    *answer.lock().unwrap() = Found::Gone;
    let n = reads.load(Ordering::SeqCst);
    assert_eq!(after(n + 3), Some(Disk::Gone));
    assert_eq!(
        told.load(Ordering::SeqCst),
        2,
        "told once more: unchanged to gone"
    );
}

#[test]
fn each_finding_has_the_word_status_writes() {
    for (disk, word) in [
        (Disk::Unchanged, "unchanged"),
        (Disk::Changed, "changed"),
        (Disk::Gone, "gone"),
        (Disk::Unreadable, "unreadable"),
    ] {
        assert_eq!(disk.word(), word);
    }
}
