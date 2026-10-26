//! M4.3: `cache_clear`'s work, the cache's own ([`Cache::clear`], decision 25): every entry
//! in the directory removed (this cache's, and those of other platforms and secrets beside
//! them), and nothing else: not the cache's `.gitignore`, its subdirectories, a write's
//! partial file, a stray file, a name like an entry's that isn't one, or anything through a
//! link, whose target is left as it was. An entry removed is only a miss; a clear while
//! another cache (another process's) rebuilds leaves no entry it trusts half-written (M2.5's
//! sealed entries, tested as its corrupt entries are: every model built during the race,
//! and after it from what's left, is the right shape). Expected values are counts by hand:
//! the example's four steps (a box, a pin, its move, the cut), an entry each.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::cache::{
    HOLE, Scratch, cache, calls, example_with_pin, files, hole, secret_of, session,
};
use super::example::EXAMPLE;
use super::model::{checked, published};
use silico_core::cache::{Cache, ClearFailure, ClearStep};
use silico_core::fake::FakeKernel;
use silico_core::recompute::Recompute;

/// An entry's name in a subdirectory: 62 lowercase hex digits.
const NAME: &str = "00000000000000000000000000000000000000000000000000000000000000";

/// The cache in `dir` holding the example's four entries, made by a session that's gone.
fn seeded(dir: &Path) -> Vec<PathBuf> {
    published(&session(dir), checked(EXAMPLE));
    let entries = files(dir);
    assert_eq!(entries.len(), 4, "{entries:?}");
    entries
}

#[test]
fn a_clear_removes_every_entry_and_says_how_many() {
    let dir = Scratch::new("clear-every");
    seeded(&dir.0);
    // Another platform's four, beside them, under another secret: entries all the same.
    let elsewhere = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::for_platform(
            &dir.0,
            "elsewhere-riscv64",
            &secret_of(&dir.0.join("other")),
        ),
    );
    published(&elsewhere, checked(EXAMPLE));
    assert_eq!(files(&dir.0).len(), 8);
    // Cleared by a cache that never wrote, of yet another secret: 4 + 4.
    let clearing = Cache::with_secret(&dir.0, &secret_of(&dir.0.join("third")));
    assert_eq!(clearing.clear().unwrap(), 8);
    assert!(files(&dir.0).is_empty(), "{:?}", files(&dir.0));
    // A second clear finds none.
    assert_eq!(clearing.clear().unwrap(), 0);
    // An entry removed is only a miss: the next session makes each step again, the same.
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    assert_eq!(
        calls(&next),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    assert_eq!(files(&dir.0).len(), 4);
}

#[test]
fn a_clear_leaves_every_file_that_isnt_an_entry_and_follows_no_link() {
    let dir = Scratch::new("clear-only-entries");
    let entries = seeded(&dir.0);
    let root = &dir.0;
    let sub = entries[0].parent().unwrap().to_owned();
    // What lies outside the cache, which links point at.
    let outside = Scratch::new("clear-outside");
    std::fs::create_dir_all(outside.0.join("ab")).unwrap();
    let target = outside.0.join("work.txt");
    std::fs::write(&target, b"the user's work").unwrap();
    let outside_entry = outside.0.join("ab").join(NAME);
    std::fs::write(&outside_entry, b"an entry's name, outside").unwrap();
    // Stray files, names like an entry's that aren't, a link where an entry goes, a
    // subdirectory that's a link out, a partial file of a write under way, and the
    // cache's .gitignore.
    let stray = root.join("notes.txt");
    std::fs::write(&stray, b"mine").unwrap();
    let at_root = root.join(NAME);
    std::fs::write(&at_root, b"an entry's name, at the top").unwrap();
    let short = sub.join(&NAME[1..]);
    std::fs::write(&short, b"61 digits").unwrap();
    let long = sub.join(format!("{NAME}0"));
    std::fs::write(&long, b"63 digits").unwrap();
    let upper = sub.join(NAME.replace('0', "A"));
    std::fs::write(&upper, b"upper case").unwrap();
    let not_hex = sub.join(NAME.replace('0', "g"));
    std::fs::write(&not_hex, b"not hex").unwrap();
    let a_directory = sub.join(NAME.replace('0', "1"));
    std::fs::create_dir(&a_directory).unwrap();
    let a_link = sub.join(NAME.replace('0', "2"));
    std::os::unix::fs::symlink(&target, &a_link).unwrap();
    // At a subdirectory's name no entry of the seed's took.
    let free = (0..=255_u8)
        .map(|n| format!("{n:02x}"))
        .find(|name| !root.join(name).exists())
        .unwrap();
    let linked_sub = root.join(free);
    std::os::unix::fs::symlink(outside.0.join("ab"), &linked_sub).unwrap();
    let wrong_sub = root.join("zz");
    std::fs::create_dir(&wrong_sub).unwrap();
    let in_wrong_sub = wrong_sub.join(NAME);
    std::fs::write(&in_wrong_sub, b"in a subdirectory not named as the cache's").unwrap();
    let partial = sub.join(".partial-999999-1");
    std::fs::write(&partial, b"a write under way").unwrap();
    let gitignore = root.join(".gitignore");
    let ignored = std::fs::read(&gitignore).unwrap();

    assert_eq!(cache(root).clear().unwrap(), 4);
    for entry in &entries {
        assert!(!entry.exists(), "{entry:?} left");
    }
    for kept in [
        &stray,
        &at_root,
        &short,
        &long,
        &upper,
        &not_hex,
        &in_wrong_sub,
        &partial,
    ] {
        assert!(kept.is_file(), "{kept:?} removed");
    }
    assert!(a_directory.is_dir());
    assert!(sub.is_dir(), "the subdirectory is the cache's to keep");
    assert!(std::fs::symlink_metadata(&a_link).unwrap().is_symlink());
    assert!(std::fs::symlink_metadata(&linked_sub).unwrap().is_symlink());
    assert_eq!(std::fs::read(&target).unwrap(), b"the user's work");
    assert_eq!(
        std::fs::read(&outside_entry).unwrap(),
        b"an entry's name, outside"
    );
    assert_eq!(std::fs::read(&gitignore).unwrap(), ignored);
    // Nothing more is an entry: a second clear removes none of them.
    assert_eq!(cache(root).clear().unwrap(), 0);
    assert!(stray.is_file() && outside_entry.is_file() && partial.is_file());
}

#[test]
fn a_cache_through_a_link_or_none_at_all_clears_nothing() {
    // The directory itself a link: an entry where it points is left (the cache keeps
    // nothing through a link, so none there is its).
    let real = Scratch::new("clear-linked-real");
    seeded(&real.0);
    let link = Scratch::new("clear-linked");
    std::os::unix::fs::symlink(&real.0, &link.0).unwrap();
    assert_eq!(cache(&link.0).clear().unwrap(), 0);
    assert_eq!(files(&real.0).len(), 4);
    // So with a trailing slash, through which a link would be followed.
    let slashed = PathBuf::from(format!("{}/", link.0.display()));
    assert_eq!(cache(&slashed).clear().unwrap(), 0);
    assert_eq!(files(&real.0).len(), 4);
    // No directory, and a file in its place.
    let none = Scratch::new("clear-none");
    assert_eq!(cache(&none.0).clear().unwrap(), 0);
    std::fs::write(&none.0, b"a file").unwrap();
    assert_eq!(cache(&none.0).clear().unwrap(), 0);
    assert_eq!(std::fs::read(&none.0).unwrap(), b"a file");
    // A cache with no secret (`silico show`'s) clears all the same: what's there is the
    // directory's, whoever wrote it.
    assert_eq!(Cache::unsealed(&real.0).clear().unwrap(), 4);
}

#[test]
fn what_a_clear_cant_remove_or_read_is_said_and_the_rest_removed() {
    let dir = Scratch::new("clear-denied");
    let entries = seeded(&dir.0);
    // The subdirectories, in order of name: the first can't be written (its entry can't
    // be removed), the second can't be read; the rest clear.
    let mut subs: Vec<PathBuf> = entries
        .iter()
        .map(|e| e.parent().unwrap().to_owned())
        .collect();
    subs.sort();
    subs.dedup();
    assert!(subs.len() >= 2, "{subs:?}");
    let in_sub = |sub: &Path| entries.iter().filter(|e| e.parent() == Some(sub)).count() as u64;
    std::fs::set_permissions(&subs[0], std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::set_permissions(&subs[1], std::fs::Permissions::from_mode(0o333)).unwrap();
    if std::fs::write(subs[0].join("probe"), b"").is_ok() {
        return; // root
    }
    let unremoved = in_sub(&subs[0]);
    let unread = in_sub(&subs[1]);
    let uncleared = cache(&dir.0).clear().unwrap_err();
    // Every other entry removed.
    assert_eq!(uncleared.removed, 4 - unremoved - unread);
    // Each entry it couldn't remove, by path, then the directory it couldn't read.
    // Each entry it couldn't remove, in order of path, then the directory it couldn't read
    // (which comes after, by name).
    let said: Vec<(PathBuf, ClearStep)> = uncleared
        .failures
        .iter()
        .map(|ClearFailure { path, step, why }| {
            assert_eq!(why.raw_os_error(), Some(libc::EACCES), "{path:?}");
            (path.clone(), *step)
        })
        .collect();
    let mut want: Vec<(PathBuf, ClearStep)> = entries
        .iter()
        .filter(|e| e.parent() == Some(subs[0].as_path()))
        .map(|e| (e.clone(), ClearStep::Remove))
        .collect();
    want.sort_by(|a, b| a.0.cmp(&b.0));
    want.push((subs[1].clone(), ClearStep::Read));
    assert_eq!(said, want);
    assert_eq!(uncleared.total, unremoved + 1);
    for sub in &subs[..2] {
        std::fs::set_permissions(sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // Once it may, the next clear removes what was left.
    assert_eq!(cache(&dir.0).clear().unwrap(), unremoved + unread);
}

#[test]
fn a_clear_while_another_process_rebuilds_leaves_no_entry_trusted_half_written() {
    // Sessions of their own caches (as other processes are: each its own count of the
    // directory) rebuild three documents over and over while another clears the directory
    // as fast as it can. Every model each builds is the right shape, from an entry read whole
    // or a step made again, never from part of one; and once they're done, what's left on
    // disk is only whole entries, each taken by a last session as the right shape.
    let dir = Scratch::new("clear-race");
    let radii = ["2.5", "3", "1"];
    let stop = Arc::new(AtomicBool::new(false));
    let clearer = {
        let (dir, stop) = (dir.0.clone(), Arc::clone(&stop));
        std::thread::spawn(move || {
            let clearing = cache(&dir);
            let mut cleared = 0_u64;
            while !stop.load(Ordering::Relaxed) {
                cleared += clearing.clear().unwrap();
            }
            cleared
        })
    };
    let builders: Vec<_> = radii
        .iter()
        .map(|radius| {
            let (dir, radius) = (dir.0.clone(), (*radius).to_owned());
            std::thread::spawn(move || {
                for _ in 0..20 {
                    let engine = session(&dir);
                    let model = published(&engine, example_with_pin(&radius));
                    assert_eq!(
                        hole(&model),
                        format!(
                            "cut(box(40, 20, 10), translate(cylinder({radius}, 12), 20, 10, -1))"
                        )
                    );
                }
            })
        })
        .collect();
    for builder in builders {
        builder.join().unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    let cleared = clearer.join().unwrap();
    assert!(cleared > 0, "the clears raced nothing");
    // What's left is whole: no partial file, and each entry a hit of the right shape.
    let left = files(&dir.0);
    assert!(
        left.iter()
            .all(|p| !p.to_string_lossy().contains(".partial-")),
        "{left:?}"
    );
    for radius in radii {
        let last = session(&dir.0);
        let model = published(&last, example_with_pin(radius));
        assert_eq!(
            hole(&model),
            format!("cut(box(40, 20, 10), translate(cylinder({radius}, 12), 20, 10, -1))")
        );
    }
    // And a whole set of entries again, all hits.
    for radius in radii {
        published(&session(&dir.0), example_with_pin(radius));
    }
    let hits = session(&dir.0);
    for radius in radii {
        published(&hits, example_with_pin(radius));
    }
    assert!(calls(&hits).is_empty(), "{:?}", calls(&hits));
}
