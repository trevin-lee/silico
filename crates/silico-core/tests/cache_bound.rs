//! M2.5: the rebuild cache's bound (docs/design/m2.5-cache-bound.md). Puts past the bound
//! evict the least recently used entries, a hit renewing its entry, so the cache stays
//! within its bound and the newest entries survive; a document whose entries were evicted
//! rebuilds to the same shapes; an entry larger than the bound isn't kept; a scan removes
//! partial files an hour old and keeps a writer's, and files the cache doesn't know. Expected
//! values are written by hand: the entries' sizes on disk, the order of hand-set times, and
//! which kernel calls each session makes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::bound;
use super::cache;
use super::example;
use super::model;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use bound::{bounded, set_time, total};
use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, session};
use example::EXAMPLE;
use model::{checked, published};
use silico_core::cache::{DEFAULT_BOUND, STALE_PARTIAL};

/// The bytes one build of the example with the pin's radius `radius` keeps, alone: in a
/// directory of its own each time, since tests running as threads of one process measure
/// at once.
fn one_document(radius: &str) -> u64 {
    static MEASURES: AtomicUsize = AtomicUsize::new(0);
    let n = MEASURES.fetch_add(1, Ordering::Relaxed);
    let dir = Scratch::new(&format!("measure-{radius}-{n}"));
    published(&session(&dir.0), example_with_pin(radius));
    total(&dir.0)
}

#[test]
fn the_default_bound_keeps_the_largest_entry_twice_over() {
    // 2 GiB: twice the largest entry (1 GiB, twice the largest frame), so even that's kept.
    assert_eq!(DEFAULT_BOUND, 2 << 30);
    assert_eq!(STALE_PARTIAL, Duration::from_secs(60 * 60));
}

#[test]
fn puts_past_the_bound_keep_the_cache_within_it_and_the_newest_document_whole() {
    let dir = Scratch::new("bound");
    // Room for about four documents' entries.
    let bound = 4 * one_document("1");
    for radius in 1..=20 {
        // A session of its own for each, so each build takes the block from the disk.
        let session = bounded(&dir.0, bound);
        published(&session, example_with_pin(&radius.to_string()));
        let kept = total(&dir.0);
        assert!(
            kept <= bound,
            "{kept} bytes kept after pin {radius}, past {bound}"
        );
    }
    // The newest document is kept whole.
    let last = bounded(&dir.0, bound);
    published(&last, example_with_pin("20"));
    assert!(calls(&last).is_empty(), "{:?}", calls(&last));
    // The first lost its own steps, written long ago and never used again; its block,
    // written first but used by every build since, is kept.
    let first = bounded(&dir.0, bound);
    published(&first, example_with_pin("1"));
    assert_eq!(calls(&first), ["Cut", "MakeCylinder", "Translate"]);
}

#[test]
fn a_document_whose_entries_were_evicted_rebuilds_to_the_same_shapes() {
    let fresh = Scratch::new("evicted-fresh");
    let built = published(&session(&fresh.0), checked(EXAMPLE));
    let dir = Scratch::new("evicted");
    let bound = 3 * one_document("1");
    published(&bounded(&dir.0, bound), checked(EXAMPLE));
    for radius in 1..=12 {
        published(
            &bounded(&dir.0, bound),
            example_with_pin(&radius.to_string()),
        );
    }
    let again = bounded(&dir.0, bound);
    let rebuilt = published(&again, checked(EXAMPLE));
    // Rebuilt through the kernel (the block was renewed by every build; the rest went)...
    assert_eq!(calls(&again), ["Cut", "MakeCylinder", "Translate"]);
    // ...to the same shapes, histories, and validity, feature for feature.
    assert_eq!(rebuilt.features(), built.features());
    assert_eq!(hole(&rebuilt), HOLE);
}

#[test]
fn an_entry_larger_than_the_bound_isnt_kept_and_nothing_is_evicted_for_it() {
    let dir = Scratch::new("too-large");
    published(&session(&dir.0), checked(EXAMPLE));
    let before: Vec<(PathBuf, Vec<u8>)> = files(&dir.0)
        .into_iter()
        .map(|file| {
            let bytes = std::fs::read(&file).unwrap();
            (file, bytes)
        })
        .collect();
    assert_eq!(before.len(), 4);
    // Every entry is more than 47 bytes: the format's magic and the seal alone are.
    let session = bounded(&dir.0, 47);
    published(&session, example_with_pin("3"));
    assert_eq!(calls(&session), ["Cut", "MakeCylinder", "Translate"]);
    let after: Vec<(PathBuf, Vec<u8>)> = files(&dir.0)
        .into_iter()
        .map(|file| {
            let bytes = std::fs::read(&file).unwrap();
            (file, bytes)
        })
        .collect();
    assert_eq!(after, before);
}

/// A name the cache would give an entry: 62 hex digits, in the subdirectory `ab`.
fn planted_entry(dir: &Path, n: u64) -> PathBuf {
    dir.join("ab").join(format!("{n:062x}"))
}

#[test]
fn eviction_goes_by_last_use_then_by_path() {
    let dir = Scratch::new("order");
    std::fs::create_dir_all(dir.0.join("ab")).unwrap();
    // Eight entries of 4 KiB, 32 KiB in all: exactly the bound, so the first put passes it.
    // Their last uses, oldest first: 5; 7; 2 and 6 at once (2 first, by path); then 0, 1,
    // 3, 4. Three go, so the cut falls between the two used at once.
    let now = SystemTime::now();
    let ago = |secs: u64| now - Duration::from_secs(secs);
    let used = [
        ago(30),
        ago(29),
        ago(40),
        ago(28),
        ago(27),
        ago(50),
        ago(40),
        ago(45),
    ];
    for (n, time) in used.iter().enumerate() {
        let entry = planted_entry(&dir.0, n as u64);
        std::fs::write(&entry, vec![0; 4096]).unwrap();
        set_time(&entry, *time);
    }
    let bound = 8 * 4096;
    let session = bounded(&dir.0, bound);
    published(&session, checked(EXAMPLE));
    // The example's own entries, each under 2 KiB, so the evictions are the first put's.
    let planted: Vec<PathBuf> = (0..8).map(|n| planted_entry(&dir.0, n)).collect();
    let own: Vec<PathBuf> = files(&dir.0)
        .into_iter()
        .filter(|file| !planted.contains(file))
        .collect();
    assert_eq!(own.len(), 4, "{own:?}");
    for file in &own {
        assert!(std::fs::metadata(file).unwrap().len() < 2048, "{file:?}");
    }
    // The first put, of at most 2 KiB, leaves what's kept and itself at most three quarters
    // of the bound, 24 KiB: five planted entries stay (20 KiB), so three go, the oldest.
    let gone: Vec<u64> = (0..8).filter(|&n| !planted[n as usize].exists()).collect();
    assert_eq!(gone, [2, 5, 7]);
    assert!(total(&dir.0) <= bound);
}

#[test]
fn a_scan_removes_partials_an_hour_old_and_keeps_a_writers_and_what_it_doesnt_know() {
    let dir = Scratch::new("partials");
    published(&session(&dir.0), checked(EXAMPLE));
    let sub = files(&dir.0)[0].parent().unwrap().to_owned();
    let stale = SystemTime::now() - STALE_PARTIAL - Duration::from_secs(60);
    // A writer's partial, written a moment ago, and its root's: kept.
    let writing = sub.join(".partial-1-1");
    std::fs::write(&writing, b"being written").unwrap();
    let writing_root = dir.0.join(".partial-1-2");
    std::fs::write(&writing_root, b"being written").unwrap();
    // Killed writes, an hour and a minute old, in a subdirectory and the root: removed.
    let killed = sub.join(".partial-1-3");
    std::fs::write(&killed, b"cut short").unwrap();
    set_time(&killed, stale);
    let killed_root = dir.0.join(".partial-1-4");
    std::fs::write(&killed_root, b"cut short").unwrap();
    set_time(&killed_root, stale);
    // A stale partial that's a link: removed as a link, what it points at kept.
    let elsewhere = Scratch::new("partials-elsewhere");
    std::fs::create_dir_all(&elsewhere.0).unwrap();
    let pointed_at = elsewhere.0.join("a user's file");
    std::fs::write(&pointed_at, b"keep me").unwrap();
    let link = sub.join(".partial-1-5");
    std::os::unix::fs::symlink(&pointed_at, &link).unwrap();
    set_time(&link, stale);
    // Files the cache doesn't know, old and large: kept, and not counted (a megabyte, past
    // the bound below, would otherwise evict every entry).
    let notes = sub.join("notes");
    std::fs::write(&notes, vec![b'n'; 1 << 20]).unwrap();
    set_time(&notes, stale);
    let root_notes = dir.0.join("notes");
    std::fs::write(&root_notes, b"mine").unwrap();
    set_time(&root_notes, stale);

    let bound = 4 * one_document("3");
    let edit = bounded(&dir.0, bound);
    published(&edit, example_with_pin("3"));
    assert_eq!(calls(&edit), ["Cut", "MakeCylinder", "Translate"]);

    assert!(writing.exists() && writing_root.exists());
    assert!(!killed.exists() && !killed_root.exists());
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the link is left"
    );
    assert_eq!(std::fs::read(&pointed_at).unwrap(), b"keep me");
    assert_eq!(std::fs::metadata(&notes).unwrap().len(), 1 << 20);
    assert_eq!(std::fs::read(&root_notes).unwrap(), b"mine");
    // Nothing was evicted: both documents are kept whole.
    let both = bounded(&dir.0, bound);
    published(&both, checked(EXAMPLE));
    published(&both, example_with_pin("3"));
    assert!(calls(&both).is_empty(), "{:?}", calls(&both));
}

#[test]
fn two_sessions_on_one_directory_keep_it_within_the_bound_and_an_eighth() {
    let dir = Scratch::new("two-sessions");
    let bound = 4 * one_document("1");
    // Each session keeps its block in memory, and adds three entries a document.
    let sessions = [bounded(&dir.0, bound), bounded(&dir.0, bound)];
    for radius in 1..=30 {
        published(&sessions[radius % 2], example_with_pin(&radius.to_string()));
        let kept = total(&dir.0);
        assert!(
            kept <= bound + bound / 8,
            "{kept} bytes kept after pin {radius}, past {bound} and an eighth"
        );
    }
}

#[test]
fn a_read_only_cache_still_hits() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = Scratch::new("read-only");
    published(&session(&dir.0), checked(EXAMPLE));
    let read_only = |path: &Path, mode| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    for file in files(&dir.0) {
        read_only(&file, 0o444);
        read_only(file.parent().unwrap(), 0o555);
    }
    read_only(&dir.0, 0o555);
    let again = bounded(&dir.0, DEFAULT_BOUND);
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

#[test]
fn a_writers_partial_counts_toward_the_bound() {
    let dir = Scratch::new("partial-counts");
    published(&session(&dir.0), checked(EXAMPLE));
    let sub = files(&dir.0)[0].parent().unwrap().to_owned();
    let bound = 4 * one_document("3");
    // A writer's partial, a moment old, of all but 100 bytes of the bound: with it, the
    // next put passes the bound, and what's left must fit in three quarters of it, less
    // than the partial alone, so every entry goes, the block too (renewed by the edit's hit,
    // but used before anything the edit writes).
    std::fs::write(sub.join(".partial-1-1"), vec![0; bound as usize - 100]).unwrap();
    let edit = bounded(&dir.0, bound);
    published(&edit, example_with_pin("3"));
    assert_eq!(calls(&edit), ["Cut", "MakeCylinder", "Translate"]);
    let again = bounded(&dir.0, bound);
    published(&again, checked(EXAMPLE));
    assert_eq!(
        calls(&again),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
}

#[test]
fn one_session_building_document_after_document_keeps_within_its_bound_between_scans() {
    // Found by review: a session scans after each eighth of the bound it writes, and in
    // between counts what it writes; a put that its count says would pass the bound must
    // scan and evict then, not wait for the next eighth. One session (its block kept in
    // memory, so three entries a document), a bound of about sixteen entries, sixty
    // documents.
    let dir = Scratch::new("between-scans");
    let bound = 4 * one_document("1");
    let engine = bounded(&dir.0, bound);
    for radius in 1..=60 {
        published(&engine, example_with_pin(&radius.to_string()));
        let kept = total(&dir.0);
        assert!(
            kept <= bound,
            "{kept} bytes kept after pin {radius}, past {bound}"
        );
    }
    // And by evicting, not by refusing what comes: the newest document's own steps are
    // kept. Its block isn't: the session took it from memory after its first build, which
    // renews nothing on disk, so it was the least recently used.
    let last = bounded(&dir.0, bound);
    published(&last, example_with_pin("60"));
    assert_eq!(calls(&last), ["MakeBox"]);
}

#[test]
fn a_partial_dated_an_hour_ahead_is_stale_and_one_half_an_hour_ahead_is_kept() {
    // Found by review: a partial dated in the future (a clock that was wrong, a server's
    // ahead) was never stale, and one the size of the bound kept every put out until the
    // clock caught up. One more than an hour ahead is removed; one half an hour ahead, as
    // fresh as a clock a little off can say, is kept.
    let dir = Scratch::new("future-partial");
    published(&session(&dir.0), checked(EXAMPLE));
    let sub = files(&dir.0)[0].parent().unwrap().to_owned();
    let bound = 4 * one_document("3");
    let ahead = sub.join(".partial-1-1");
    std::fs::write(&ahead, vec![0; bound as usize]).unwrap();
    set_time(
        &ahead,
        SystemTime::now() + STALE_PARTIAL + Duration::from_secs(60),
    );
    let near = sub.join(".partial-1-2");
    std::fs::write(&near, b"being written").unwrap();
    set_time(&near, SystemTime::now() + STALE_PARTIAL / 2);
    let edit = bounded(&dir.0, bound);
    published(&edit, example_with_pin("3"));
    assert!(!ahead.exists());
    assert!(near.exists());
    // Nothing was evicted or refused: both documents are kept whole.
    let both = bounded(&dir.0, bound);
    published(&both, checked(EXAMPLE));
    published(&both, example_with_pin("3"));
    assert!(calls(&both).is_empty(), "{:?}", calls(&both));
}

#[test]
fn a_hit_renews_its_entrys_last_use() {
    // A session builds the example; every entry is dated an hour back; another session
    // builds it again, every step from the disk. Each entry it read is used now: its last
    // use is no earlier than that build's start.
    let dir = Scratch::new("renewed");
    published(&session(&dir.0), checked(EXAMPLE));
    let entries = files(&dir.0);
    assert!(!entries.is_empty());
    for entry in &entries {
        set_time(entry, SystemTime::now() - Duration::from_secs(3600));
    }
    let again = session(&dir.0);
    let start = SystemTime::now();
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
    for entry in &entries {
        let used = std::fs::metadata(entry).unwrap().modified().unwrap();
        assert!(used >= start, "{} last used {used:?}", entry.display());
    }
}
