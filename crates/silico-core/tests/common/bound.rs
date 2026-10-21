//! For the cache's bound's tests: bounded sessions, the cache's size on disk, and files'
//! times set by hand.

use std::path::Path;
use std::time::SystemTime;

use silico_core::fake::FakeKernel;
use silico_core::recompute::Recompute;

use crate::cache::{cache, files};

/// A fresh session over the cache in `dir`, bounded at `bound` bytes, on a fake of `size`.
pub fn bounded_on(dir: &Path, bound: u64, size: usize) -> Recompute<FakeKernel> {
    Recompute::with_cache(FakeKernel::new(size).unwrap(), cache(dir).with_bound(bound))
}

/// A fresh session over the cache in `dir`, bounded at `bound` bytes.
pub fn bounded(dir: &Path, bound: u64) -> Recompute<FakeKernel> {
    bounded_on(dir, bound, 2)
}

/// The bytes of every file in the cache's subdirectories, its entries and partial files,
/// by length (links' own).
pub fn total(dir: &Path) -> u64 {
    files(dir)
        .iter()
        .map(|file| std::fs::symlink_metadata(file).unwrap().len())
        .sum()
}

/// Sets `path`'s modification time, a link's own, to `secs` (and `nanos`) from the epoch,
/// negative for before it.
pub fn set_time_at(path: &Path, secs: i64, nanos: i64) {
    use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, utimensat};
    let at = Timespec {
        tv_sec: secs,
        tv_nsec: nanos,
    };
    utimensat(
        CWD,
        path,
        &Timestamps {
            last_access: at,
            last_modification: at,
        },
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .unwrap();
}

/// Sets `path`'s modification time, a link's own, to `time`.
pub fn set_time(path: &Path, time: SystemTime) {
    let since = time.duration_since(SystemTime::UNIX_EPOCH).unwrap();
    set_time_at(
        path,
        since.as_secs() as i64,
        i64::from(since.subsec_nanos()),
    );
}
