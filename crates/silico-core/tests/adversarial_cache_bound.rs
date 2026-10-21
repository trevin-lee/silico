//! break-it: the M2.5 rebuild cache's bound and eviction (docs/design/m2.5-cache-bound.md)
//! under hostile directories. Sparse files whose lengths sum past what a `u64` holds, puts
//! racing in one session's parallel branches, partials at the staleness edge and dated
//! before the epoch, names one character off an entry's, links and hard links where entries
//! go, read-only subdirectories over the bound, and bounds of 0, 1, exactly an entry, and
//! `u64::MAX`. Expected values are written by hand: the bound the design promises, which
//! kernel calls a session makes, and the fake's description of each shape.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/bound.rs"]
mod bound;
#[path = "common/cache.rs"]
mod cache;
#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use bound::{bounded, bounded_on, set_time, set_time_at, total};
use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, session};
use example::EXAMPLE;
use model::{checked, published};
use silico_core::cache::{DEFAULT_BOUND, STALE_PARTIAL};
use silico_core::document::Checked;

/// A name the cache would give an entry: 62 hex digits, in the subdirectory `sub`.
fn entry_name(dir: &Path, sub: &str, n: u64) -> PathBuf {
    dir.join(sub).join(format!("{n:062x}"))
}

/// The longest sparse file this file system takes at `path`, found by halving: APFS takes
/// about 2^55 bytes, ext4 2^44, tmpfs 2^63.
fn longest_sparse(path: &Path) -> u64 {
    let file = std::fs::File::create(path).unwrap();
    let (mut lo, mut hi) = (1u64 << 30, i64::MAX as u64);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if file.set_len(mid).is_ok() {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    file.set_len(lo).unwrap();
    lo
}

/// How many sparse files of `longest` bytes sum past `u64::MAX`, if that's few enough to
/// make: on APFS (2^55 bytes a file) 513, on tmpfs 3. On ext4 (2^44) it's a million, too
/// many, so none, and the tests using it test nothing there: the unit tests in cache.rs
/// evict from such sums on any system.
fn past_u64_max(longest: u64) -> Option<u64> {
    let count = u64::MAX / longest + 2;
    (count <= 4096).then_some(count)
}

/// The example with each of its features' solids in the cache: the published model's hole.
fn seed(dir: &Path) {
    assert_eq!(hole(&published(&session(dir), checked(EXAMPLE))), HOLE);
}

// ---------------------------------------------------------------------------------------
// Sizes past what a u64 holds.

// Found by break-it: `scan` added each file's length with a plain `+=`. Sparse files cost no
// disk, so a directory that comes with a document (a tarball, a copied cache) can hold
// entries whose lengths sum past `u64::MAX`: 513 files of APFS's longest (2^55 - 1 bytes).
// In a debug build the add panicked in `put`, outside the job's `catch_unwind`, and the
// recompute failed with an Internal error: the document couldn't be rebuilt in that
// directory at all. In release it wrapped, and counted the directory as nearly empty. (A
// saturating add isn't enough either: it undercounts, and evicting to the count it reaches
// leaves a planted entry.) The scan counts in a `u128` now. Expected: every planted file,
// each past the bound and older than any entry the session writes, is evicted, and the
// example builds as it always does.
#[test]
fn sparse_entries_whose_lengths_sum_past_u64_max_are_evicted_and_the_document_builds() {
    let dir = Scratch::new("u64-entries");
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    let first = entry_name(&dir.0, "ab", 0);
    let longest = longest_sparse(&first);
    let Some(count) = past_u64_max(longest) else {
        return;
    };
    let planted: Vec<PathBuf> = (0..count).map(|n| entry_name(&dir.0, "ab", n)).collect();
    for entry in &planted {
        std::fs::File::create(entry)
            .unwrap()
            .set_len(longest)
            .unwrap();
        set_time(entry, SystemTime::now() - Duration::from_secs(86_400));
    }
    let engine = bounded(&dir.0, DEFAULT_BOUND);
    assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
    assert!(planted.iter().all(|entry| !entry.exists()));
    assert!(total(&dir.0) <= DEFAULT_BOUND);
}

// Found by break-it: the same overflow through partial files. Fresh partials, a concurrent
// writer's by their times, are counted and left. Expected: the example builds, recompute
// after recompute (what's kept is moot while the partials take more than the bound).
#[test]
fn fresh_sparse_partials_whose_lengths_sum_past_u64_max_dont_stop_the_recompute() {
    let dir = Scratch::new("u64-partials");
    std::fs::create_dir_all(&dir.0).unwrap();
    let longest = longest_sparse(&dir.0.join(".partial-1-0"));
    let Some(count) = past_u64_max(longest) else {
        return;
    };
    for n in 1..count {
        std::fs::File::create(dir.0.join(format!(".partial-1-{n}")))
            .unwrap()
            .set_len(longest)
            .unwrap();
    }
    let engine = bounded(&dir.0, DEFAULT_BOUND);
    assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
    // And a second recompute, after the first's puts, still ends.
    assert_eq!(
        hole(&published(&engine, example_with_pin("3"))),
        "cut(box(40, 20, 10), translate(cylinder(3, 12), 20, 10, -1))"
    );
}

#[test]
fn one_sparse_entry_of_the_longest_length_is_evicted_and_the_document_kept() {
    // Under u64::MAX, a single huge planted entry is a planted entry like any other: it's
    // the oldest, past the bound, so it goes, and the example's entries all stay.
    let dir = Scratch::new("one-huge");
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    let huge = entry_name(&dir.0, "ab", 7);
    longest_sparse(&huge);
    set_time(&huge, SystemTime::now() - Duration::from_secs(86_400));
    published(&bounded(&dir.0, DEFAULT_BOUND), checked(EXAMPLE));
    assert!(!huge.exists());
    let again = bounded(&dir.0, DEFAULT_BOUND);
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

// ---------------------------------------------------------------------------------------
// One session's parallel puts.

/// `count` boxes, none consumed, each a leaf the recompute may run beside the others, all
/// of one size of description (lengths 10 to 10 + count - 1, two digits each).
fn boxes(count: usize) -> Checked {
    let features: Vec<String> = (0..count)
        .map(|n| {
            format!(
                r#"    {{"id": "bx{n:06}", "type": "box", "v": 1, "name": "Box {n}", "position": [0, 0, 0], "length": {}, "width": 20, "height": 10}}"#,
                10 + n
            )
        })
        .collect();
    checked(&format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"features\": [\n{}\n  ]\n}}\n",
        features.join(",\n")
    ))
}

// Found by break-it (44 to 55 of 60 trials, up to all eight entries, 8x the bound): the
// design says the bound is exact for one session, but `make_room` only evicted what a scan
// sees, and never refused to write. An entry another of the session's job threads is
// writing (counted in `writing`, not yet renamed into place) can't be evicted, so every
// put at once was written. A put that can't be made room for is refused now. Eight leaves on
// an eight-worker fake, each a box whose entry is within a few bytes of the others', bounded
// at the largest: one after another, each put evicts every other entry (what's left must
// fit in three quarters of the bound less the entry, under a quarter of it), so one entry is
// kept, as the control `one_sessions_puts_one_after_another_keep_the_cache_within_its_bound`
// shows. Expected: at most the bound after every recompute.
#[test]
fn one_sessions_parallel_puts_keep_the_cache_within_its_bound() {
    const LEAVES: usize = 8;
    let document = boxes(LEAVES);
    let measure = Scratch::new("parallel-measure");
    published(&session(&measure.0), document.clone());
    let sizes: Vec<u64> = files(&measure.0)
        .iter()
        .map(|file| std::fs::metadata(file).unwrap().len())
        .collect();
    assert_eq!(sizes.len(), LEAVES, "{sizes:?}");
    let bound = *sizes.iter().max().unwrap();
    let least = *sizes.iter().min().unwrap();
    assert!(least > bound / 2, "{sizes:?}");
    const TRIALS: usize = 60;
    let mut past = Vec::new();
    for trial in 0..TRIALS {
        let dir = Scratch::new(&format!("parallel-{trial}"));
        let engine = bounded_on(&dir.0, bound, LEAVES);
        published(&engine, document.clone());
        drop(engine);
        let kept = total(&dir.0);
        if kept > bound {
            past.push(kept);
        }
    }
    assert!(
        past.is_empty(),
        "{} of {TRIALS} sessions' parallel puts left more than their bound of {bound} bytes: {past:?}",
        past.len()
    );
}

// ---------------------------------------------------------------------------------------
// Partials at the edge of stale, and dated oddly.

#[test]
fn a_partial_a_minute_short_of_stale_is_kept_and_a_minute_past_is_removed() {
    let dir = Scratch::new("stale-edge");
    seed(&dir.0);
    let sub = files(&dir.0)[0].parent().unwrap().to_owned();
    let now = SystemTime::now();
    let short = sub.join(".partial-1-1");
    std::fs::write(&short, b"almost stale").unwrap();
    set_time(&short, now - STALE_PARTIAL + Duration::from_secs(60));
    let past = sub.join(".partial-1-2");
    std::fs::write(&past, b"just stale").unwrap();
    set_time(&past, now - STALE_PARTIAL - Duration::from_secs(60));
    let edit = bounded(&dir.0, DEFAULT_BOUND);
    published(&edit, example_with_pin("3"));
    assert!(short.exists());
    assert!(!past.exists());
}

#[test]
fn a_partial_dated_before_the_epoch_is_stale_and_removed() {
    let dir = Scratch::new("pre-epoch-partial");
    seed(&dir.0);
    let partial = dir.0.join(".partial-1-1");
    std::fs::write(&partial, b"from 1960").unwrap();
    set_time_at(&partial, -315_619_200, 0);
    published(&bounded(&dir.0, DEFAULT_BOUND), example_with_pin("3"));
    assert!(!partial.exists());
}

#[test]
fn entries_dated_before_the_epoch_are_evicted_first() {
    // Last uses before 1970 (a clock set back, a restored backup) are the oldest, and go
    // first: the example's own entries, written now, stay.
    let dir = Scratch::new("pre-epoch-entries");
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    let old: Vec<PathBuf> = (0..4).map(|n| entry_name(&dir.0, "ab", n)).collect();
    for (n, entry) in old.iter().enumerate() {
        std::fs::write(entry, vec![0; 4096]).unwrap();
        set_time_at(entry, -1_000_000 - n as i64, 999_999_999);
    }
    // Room for two planted entries and the example (each of its entries under 2 KiB).
    let bound = 4 * 4096;
    let engine = bounded(&dir.0, bound);
    published(&engine, checked(EXAMPLE));
    // The first put leaves at most three quarters of the bound, 12 KiB, with itself: two
    // planted entries go, the two oldest (the most negative times, 3 then 2).
    let gone: Vec<usize> = (0..4).filter(|&n| !old[n].exists()).collect();
    assert_eq!(gone, [2, 3]);
    let again = bounded(&dir.0, bound);
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

#[test]
fn a_stale_partial_thats_a_directory_or_a_fifo_is_handled_as_its_kind() {
    // A directory named as a partial is never removed (it may hold anything) and not counted;
    // a FIFO named as one, stale, is removed without being opened.
    let dir = Scratch::new("partial-kinds");
    seed(&dir.0);
    let stale = SystemTime::now() - STALE_PARTIAL - Duration::from_secs(60);
    let as_dir = dir.0.join(".partial-1-1");
    std::fs::create_dir(&as_dir).unwrap();
    std::fs::write(as_dir.join("inside"), b"keep").unwrap();
    set_time(&as_dir, stale);
    let fifo = dir.0.join(".partial-1-2");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    set_time(&fifo, stale);
    published(&bounded(&dir.0, DEFAULT_BOUND), example_with_pin("3"));
    assert_eq!(std::fs::read(as_dir.join("inside")).unwrap(), b"keep");
    assert!(
        std::fs::symlink_metadata(&fifo).is_err(),
        "the FIFO is left"
    );
}

// ---------------------------------------------------------------------------------------
// Names and kinds one step off an entry's.

#[test]
fn files_named_almost_as_entries_are_neither_counted_nor_evicted() {
    // Upper-case hex, 61 and 63 digits, a non-hex digit, a three-digit subdirectory, and an
    // upper-case one: none is an entry, so each is kept, and their megabytes (past the bound)
    // evict nothing.
    let dir = Scratch::new("almost-entries");
    seed(&dir.0);
    let old = SystemTime::now() - Duration::from_secs(86_400);
    let big = vec![b'x'; 1 << 20];
    let mut planted = Vec::new();
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    std::fs::create_dir_all(dir.0.join("abc")).unwrap();
    std::fs::create_dir_all(dir.0.join("AB")).unwrap();
    for name in [
        format!("ab/{}", "A".repeat(62)),
        format!("ab/{}", "a".repeat(61)),
        format!("ab/{}", "a".repeat(63)),
        format!("ab/{}g", "a".repeat(61)),
        format!("abc/{}", "a".repeat(62)),
        format!("AB/{}", "a".repeat(62)),
    ] {
        let path = dir.0.join(name);
        std::fs::write(&path, &big).unwrap();
        set_time(&path, old);
        planted.push(path);
    }
    let bound = 1 << 20;
    let both = bounded(&dir.0, bound);
    published(&both, example_with_pin("3"));
    for path in &planted {
        assert_eq!(std::fs::metadata(path).unwrap().len(), 1 << 20, "{path:?}");
    }
    // Nothing of the cache's own was evicted for them.
    let again = bounded(&dir.0, bound);
    published(&again, checked(EXAMPLE));
    published(&again, example_with_pin("3"));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

#[test]
fn a_subdirectory_thats_a_link_is_neither_counted_nor_evicted_through() {
    // A two-hex-digit link to a user's directory of files named as entries, old and large:
    // never followed, so never counted (the example's entries stay) and never removed.
    let dir = Scratch::new("linked-sub");
    seed(&dir.0);
    let outside = Scratch::new("linked-sub-outside");
    std::fs::create_dir_all(&outside.0).unwrap();
    let victims: Vec<PathBuf> = (0..3)
        .map(|n| outside.0.join(format!("{n:062x}")))
        .collect();
    for victim in &victims {
        std::fs::write(victim, vec![b'v'; 1 << 20]).unwrap();
        set_time(victim, SystemTime::now() - Duration::from_secs(86_400));
    }
    // A name the example's entries don't use.
    let used: Vec<String> = std::fs::read_dir(&dir.0)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let free = (0..=255u8)
        .map(|n| format!("{n:02x}"))
        .find(|name| !used.contains(name))
        .unwrap();
    std::os::unix::fs::symlink(&outside.0, dir.0.join(&free)).unwrap();
    let bound = 1 << 20;
    published(&bounded(&dir.0, bound), example_with_pin("3"));
    for victim in &victims {
        assert_eq!(std::fs::metadata(victim).unwrap().len(), 1 << 20);
    }
    let again = bounded(&dir.0, bound);
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

#[test]
fn entries_that_are_links_fifos_or_directories_are_neither_counted_nor_removed() {
    let dir = Scratch::new("entry-kinds");
    seed(&dir.0);
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    let outside = Scratch::new("entry-kinds-outside");
    std::fs::create_dir_all(&outside.0).unwrap();
    let target = outside.0.join("big");
    std::fs::write(&target, vec![b't'; 1 << 20]).unwrap();
    let link = entry_name(&dir.0, "ab", 1);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let fifo = entry_name(&dir.0, "ab", 2);
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let sub = entry_name(&dir.0, "ab", 3);
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("inside"), vec![b'd'; 1 << 20]).unwrap();
    let old = SystemTime::now() - Duration::from_secs(86_400);
    for path in [&link, &fifo, &sub] {
        set_time(path, old);
    }
    let bound = 1 << 19;
    published(&bounded(&dir.0, bound), example_with_pin("3"));
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::metadata(&target).unwrap().len(), 1 << 20);
    assert!(std::fs::symlink_metadata(&fifo).is_ok());
    assert!(sub.join("inside").exists());
    let again = bounded(&dir.0, bound);
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

#[test]
fn an_entry_hard_linked_to_a_users_file_is_evicted_as_a_link_and_the_file_kept() {
    let dir = Scratch::new("hard-link");
    seed(&dir.0);
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    let outside = Scratch::new("hard-link-outside");
    std::fs::create_dir_all(&outside.0).unwrap();
    let users = outside.0.join("thesis.tex");
    std::fs::write(&users, vec![b'u'; 1 << 20]).unwrap();
    let entry = entry_name(&dir.0, "ab", 9);
    std::fs::hard_link(&users, &entry).unwrap();
    set_time(&entry, SystemTime::now() - Duration::from_secs(86_400));
    published(&bounded(&dir.0, 1 << 20), example_with_pin("3"));
    assert!(!entry.exists());
    assert_eq!(std::fs::read(&users).unwrap(), vec![b'u'; 1 << 20]);
}

#[test]
fn an_unreadable_planted_entry_is_counted_and_evicted() {
    // Mode 000 stops reads, not removal (which the directory's mode decides).
    let dir = Scratch::new("mode-000");
    seed(&dir.0);
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    let entry = entry_name(&dir.0, "ab", 5);
    std::fs::write(&entry, vec![0; 1 << 20]).unwrap();
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o000)).unwrap();
    set_time(&entry, SystemTime::now() - Duration::from_secs(86_400));
    published(&bounded(&dir.0, 1 << 20), example_with_pin("3"));
    assert!(std::fs::symlink_metadata(&entry).is_err());
}

#[test]
fn a_read_only_subdirectory_over_the_bound_fails_nothing_and_the_rest_is_evicted() {
    // Old entries that can't be removed (their subdirectory is read-only) keep the cache
    // past its bound: every put scans and tries again; the recompute still builds, and what
    // can go, goes.
    let dir = Scratch::new("read-only-over");
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    std::fs::create_dir_all(dir.0.join("cd")).unwrap();
    let stuck: Vec<PathBuf> = (0..2).map(|n| entry_name(&dir.0, "ab", n)).collect();
    let movable: Vec<PathBuf> = (0..2).map(|n| entry_name(&dir.0, "cd", n)).collect();
    let old = SystemTime::now() - Duration::from_secs(86_400);
    for path in stuck.iter().chain(&movable) {
        std::fs::write(path, vec![0; 1 << 16]).unwrap();
        set_time(path, old);
    }
    std::fs::set_permissions(dir.0.join("ab"), std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(dir.0.join("ab").join("probe"), b"").is_ok() {
        // Running as root: permissions don't bite.
        return;
    }
    let bound = 1 << 16;
    let engine = bounded(&dir.0, bound);
    for radius in ["1", "2", "3"] {
        assert_eq!(
            hole(&published(&engine, example_with_pin(radius))),
            format!("cut(box(40, 20, 10), translate(cylinder({radius}, 12), 20, 10, -1))")
        );
    }
    assert!(stuck.iter().all(|path| path.exists()));
    assert!(movable.iter().all(|path| !path.exists()));
    // The stuck entries alone are past the bound, so no put found room: nothing new was
    // written (a removal that failed isn't room made).
    assert_eq!(files(&dir.0), stuck);
}

// ---------------------------------------------------------------------------------------
// Bounds at their edges.

/// A document of one box, so one entry.
fn one_box() -> Checked {
    boxes(1)
}

#[test]
fn a_bound_of_exactly_an_entry_keeps_it_and_one_byte_less_doesnt() {
    let measure = Scratch::new("exact-measure");
    published(&session(&measure.0), one_box());
    let entries = files(&measure.0);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let size = std::fs::metadata(&entries[0]).unwrap().len();

    let exact = Scratch::new("exact");
    published(&bounded(&exact.0, size), one_box());
    let again = bounded(&exact.0, size);
    published(&again, one_box());
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));

    let under = Scratch::new("exact-under");
    published(&bounded(&under.0, size - 1), one_box());
    assert!(files(&under.0).is_empty(), "{:?}", files(&under.0));
}

#[test]
fn bounds_of_zero_and_one_keep_nothing_and_fail_nothing() {
    for bound in [0, 1] {
        let dir = Scratch::new(&format!("tiny-{bound}"));
        let engine = bounded(&dir.0, bound);
        assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
        assert!(files(&dir.0).is_empty(), "{:?}", files(&dir.0));
        // And again, the same session: nothing to hit, so built again, the same.
        let again = bounded(&dir.0, bound);
        assert_eq!(hole(&published(&again, checked(EXAMPLE))), HOLE);
        assert_eq!(
            calls(&again),
            ["Cut", "MakeBox", "MakeCylinder", "Translate"]
        );
    }
}

#[test]
fn one_sessions_puts_one_after_another_keep_the_cache_within_its_bound() {
    // The control for the two tests around it: one box a document, so one put a recompute,
    // never two at once. Each put evicts the entry before it (what's left must fit in three
    // quarters of the bound less the entry), so one entry is kept, the newest.
    let one = |n: usize| {
        checked(&format!(
            "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"features\": [\n    {{\"id\": \"bx000000\", \"type\": \"box\", \"v\": 1, \"name\": \"Box\", \"position\": [0, 0, 0], \"length\": {}, \"width\": 20, \"height\": 10}}\n  ]\n}}\n",
            10 + n
        ))
    };
    let measure = Scratch::new("sequential-measure");
    published(&session(&measure.0), one(0));
    let bound = total(&measure.0);
    let dir = Scratch::new("sequential");
    let engine = bounded(&dir.0, bound);
    for n in 0..8 {
        published(&engine, one(n));
        assert_eq!(files(&dir.0).len(), 1, "after box {n}: {:?}", files(&dir.0));
        assert!(total(&dir.0) <= bound);
    }
}

// Found by break-it: the same root cause as
// `one_sessions_parallel_puts_keep_the_cache_within_its_bound`, with a kernel of one worker:
// the kernel's calls are one at a time, but each job puts in its own thread after its
// dependents are woken, so one job's put overlaps the next's. Expected: at most the bound
// after every recompute.
#[test]
fn a_one_worker_kernels_puts_keep_the_cache_within_its_bound() {
    let measure = Scratch::new("small-measure");
    published(&session(&measure.0), boxes(4));
    let largest = files(&measure.0)
        .iter()
        .map(|file| std::fs::metadata(file).unwrap().len())
        .max()
        .unwrap();
    let dir = Scratch::new("small");
    for round in 0..20 {
        let engine = bounded_on(&dir.0, largest, 1);
        published(&engine, boxes(4));
        let kept = total(&dir.0);
        assert!(
            kept <= largest,
            "round {round}: {kept} bytes kept, bound {largest}"
        );
    }
}

#[test]
fn a_bound_of_u64_max_keeps_everything_and_overflows_nothing() {
    let dir = Scratch::new("max-bound");
    for radius in ["1", "2", "3", "4"] {
        published(&bounded(&dir.0, u64::MAX), example_with_pin(radius));
    }
    // A block and three entries for each pin: every one kept.
    assert_eq!(files(&dir.0).len(), 13, "{:?}", files(&dir.0));
    let again = bounded(&dir.0, u64::MAX);
    for radius in ["1", "2", "3", "4"] {
        published(&again, example_with_pin(radius));
    }
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

// ---------------------------------------------------------------------------------------
// Sessions at once, and eviction under a reader.

/// Eight sessions at once, each on its own pins, for 20 rounds, over a bound of `documents`
/// times the example's entries: every model is right, and after every round the directory
/// holds at most the bound and `allowance(bound, document)` for each other session, where
/// `document` is the bytes of the example's entries. No partial is left.
fn sessions_at_once(documents: u64, allowance: fn(u64, u64) -> u64) {
    let dir = Scratch::new(&format!("many-sessions-{documents}"));
    let measure = Scratch::new(&format!("many-sessions-measure-{documents}"));
    published(&session(&measure.0), example_with_pin("1"));
    let document = total(&measure.0);
    let bound = documents * document;
    const SESSIONS: u64 = 8;
    let most = bound + (SESSIONS - 1) * allowance(bound, document);
    for round in 0..20 {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(SESSIONS as usize));
        let threads: Vec<_> = (0..SESSIONS)
            .map(|n| {
                let dir = dir.0.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let engine = bounded(&dir, bound);
                    barrier.wait();
                    let radius = format!("{}", 1 + n + SESSIONS * round);
                    let model = published(&engine, example_with_pin(&radius));
                    (radius, hole(&model))
                })
            })
            .collect();
        for thread in threads {
            let (radius, hole) = thread.join().unwrap();
            assert_eq!(
                hole,
                format!("cut(box(40, 20, 10), translate(cylinder({radius}, 12), 20, 10, -1))")
            );
        }
        let kept = total(&dir.0);
        assert!(
            kept <= most,
            "round {round}: {kept} bytes, bound {bound}, at most {most}"
        );
    }
    assert!(
        files(&dir.0)
            .iter()
            .all(|p| !p.to_string_lossy().contains(".partial-")),
        "{:?}",
        files(&dir.0)
    );
}

#[test]
fn sessions_putting_at_once_stay_within_an_eighth_of_the_bound_for_each_other() {
    // A bound of 16 documents, so a document's entries, every one a session writes at
    // once, are within an eighth of it: the design's promise is the bound and an eighth of
    // it for each other session. 20 rounds write 160 documents, ten times the bound, so
    // eviction runs throughout.
    sessions_at_once(16, |bound, _| bound / 8);
}

// Found by break-it: over a bound of two documents, each entry is more than an eighth of the
// bound, and a session's puts at once (the example's block and pin are built, and put, side
// by side) went past the bound by up to all of them, since no scan can evict an entry being
// written. A put that can't be made room for is refused now, so each session keeps its own
// view within the bound; but another session's puts at once, made before it sees them, can
// still be entries past an eighth of the bound. The design's promise for such entries is
// the bound and, for each other session, what it writes at once: at most its document's
// entries, built and then measured (a feature's measured copy is put again, perhaps while
// its built one is still being written), twice the document.
#[test]
fn sessions_putting_at_once_on_a_small_bound_build_right_and_stay_near_it() {
    sessions_at_once(2, |bound, document| (bound / 8).max(2 * document));
}

#[test]
fn an_entry_evicted_by_another_session_is_rebuilt_to_the_same_shape() {
    // One session's entries evicted under it by another's puts: its next recompute of a new
    // document (its block from memory, the rest rebuilt) is right, and a third session that
    // must rebuild everything gets the same shapes.
    let dir = Scratch::new("evicted-under");
    let bound = {
        let measure = Scratch::new("evicted-under-measure");
        published(&session(&measure.0), checked(EXAMPLE));
        total(&measure.0) + 64
    };
    let first = bounded(&dir.0, bound);
    published(&first, checked(EXAMPLE));
    let other = bounded(&dir.0, bound);
    for radius in ["1", "2", "3", "4", "5"] {
        published(&other, example_with_pin(radius));
    }
    assert_eq!(hole(&published(&first, checked(EXAMPLE))), HOLE);
    let third = bounded(&dir.0, bound);
    assert_eq!(hole(&published(&third, checked(EXAMPLE))), HOLE);
}
