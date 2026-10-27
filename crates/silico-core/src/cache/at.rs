//! The rebuild cache's file operations, each on a name in a directory the cache holds open
//! (the system's `*at` calls, through rustix's safe API): a name is looked up in that
//! directory, whatever has since been done to the paths that led to it, and its own name is
//! never followed through a link.

use std::ffi::CString;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, SeekFrom, Stat};
use rustix::io::Errno;
use rustix::path::Arg;

/// The directory `name` in `dir` (or at the path `name`, from [`CWD`]), opened to list it
/// and to find what's in it: never a link at its own name (refused, `ELOOP` or `ENOTDIR` as
/// the system has it).
pub(super) fn open_dir(dir: impl AsFd, name: impl Arg) -> rustix::io::Result<OwnedFd> {
    rustix::fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
}

/// Whether the directory `dir` holds open is the one at `path` now, its own name not
/// followed: the same file on the same device.
pub(super) fn is_at(dir: impl AsFd, path: &Path) -> bool {
    match (
        rustix::fs::fstat(dir),
        rustix::fs::statat(CWD, path, AtFlags::SYMLINK_NOFOLLOW),
    ) {
        (Ok(open), Ok(there)) => (open.st_dev, open.st_ino) == (there.st_dev, there.st_ino),
        _ => false,
    }
}

/// What the cache reads of a file's metadata: the file's own, a link's never followed.
#[derive(Debug, Clone, Copy)]
pub(super) struct Meta {
    kind: FileType,
    len: u64,
    modified: SystemTime,
}

impl Meta {
    fn of(stat: &Stat) -> Self {
        // Each as the system gives it; a time it can't be (before the epoch by more than a
        // `SystemTime` holds, or nanoseconds past a second) is the epoch, as a time the
        // system won't give is.
        let nanos = u32::try_from(stat.st_mtime_nsec)
            .ok()
            .filter(|nanos| *nanos < 1_000_000_000)
            .map_or(Duration::ZERO, |nanos| {
                Duration::from_nanos(u64::from(nanos))
            });
        let modified = match u64::try_from(stat.st_mtime) {
            Ok(seconds) => SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds)),
            Err(_) => SystemTime::UNIX_EPOCH
                .checked_sub(Duration::from_secs(stat.st_mtime.unsigned_abs())),
        }
        .and_then(|seconds| seconds.checked_add(nanos))
        .unwrap_or(SystemTime::UNIX_EPOCH);
        Meta {
            kind: FileType::from_raw_mode(stat.st_mode),
            len: u64::try_from(stat.st_size).unwrap_or(0),
            modified,
        }
    }

    pub(super) fn is_dir(&self) -> bool {
        self.kind == FileType::Directory
    }

    pub(super) fn is_file(&self) -> bool {
        self.kind == FileType::RegularFile
    }

    pub(super) fn is_link(&self) -> bool {
        self.kind == FileType::Symlink
    }

    pub(super) fn len(&self) -> u64 {
        self.len
    }

    pub(super) fn modified(&self) -> SystemTime {
        self.modified
    }
}

/// The metadata of `name` in `dir`, a link's own.
pub(super) fn stat(dir: impl AsFd, name: impl Arg) -> std::io::Result<Meta> {
    let stat = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
    Ok(Meta::of(&stat))
}

/// Whether `name` in `dir` is a symbolic link (not followed).
pub(super) fn is_link(dir: impl AsFd, name: impl Arg) -> bool {
    stat(dir, name).is_ok_and(|meta| meta.is_link())
}

/// The names in the directory `dir` holds open (but `.` and `..`), in order of name, listed
/// from its start; and, if it couldn't be listed whole (or at all), why. A directory removed
/// as it's listed (another process's clear) lists nothing, without a word.
pub(super) fn names(dir: impl AsFd) -> (Vec<CString>, Option<std::io::Error>) {
    // Listed from a copy of the handle, which stays the caller's: the copy shares its
    // offset, so it's set to the start first (a directory listed before is listed again
    // whole). Never reopened by a name, not even its own `.`, which a directory that may be
    // read but not searched couldn't look up.
    let listing = rustix::io::dup(dir).and_then(|copy| {
        rustix::fs::seek(&copy, SeekFrom::Start(0))?;
        Dir::new(copy)
    });
    let mut listing = match listing {
        Ok(listing) => listing,
        Err(Errno::NOENT) => return (Vec::new(), None),
        Err(e) => return (Vec::new(), Some(e.into())),
    };
    let mut names = Vec::new();
    let mut unread = None;
    while let Some(item) = listing.read() {
        match item {
            Ok(item) => {
                let name = item.file_name();
                if name != c"." && name != c".." {
                    names.push(name.to_owned());
                }
            }
            // Removed since it was opened: nothing more is in it (Linux says so, `ENOENT`).
            Err(Errno::NOENT) => break,
            Err(e) => {
                unread = Some(e.into());
                break;
            }
        }
    }
    names.sort();
    (names, unread)
}

/// Removes the file (or link, never followed) `name` in `dir`: whether this removed it, not
/// another process first.
pub(super) fn remove(dir: impl AsFd, name: impl Arg) -> std::io::Result<bool> {
    match rustix::fs::unlinkat(dir, name, AtFlags::empty()) {
        Ok(()) => Ok(true),
        Err(Errno::NOENT) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Renames `from` in `dir` to `to` in it, over anything at `to` but a directory.
pub(super) fn rename(dir: impl AsFd, from: impl Arg, to: impl Arg) -> std::io::Result<()> {
    let dir = dir.as_fd();
    rustix::fs::renameat(dir, from, dir, to).map_err(Into::into)
}

/// The file `name` in `dir`, opened to read without following a symbolic link or waiting on
/// a pipe.
pub(super) fn open_to_read(dir: impl AsFd, name: impl Arg) -> rustix::io::Result<std::fs::File> {
    rustix::fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(std::fs::File::from)
}

/// A new file in `dir`, with permissions `mode`, and its name: a partial file's, never one
/// already there (a link planted at the name would otherwise be followed, and what it
/// points at written over).
pub(super) fn new_file(dir: impl AsFd, mode: Mode) -> std::io::Result<(String, std::fs::File)> {
    static FILES: AtomicU64 = AtomicU64::new(0);
    let dir = dir.as_fd();
    let mut attempt = 0;
    loop {
        let partial = format!(
            ".partial-{}-{}",
            std::process::id(),
            FILES.fetch_add(1, Ordering::Relaxed)
        );
        let created = rustix::fs::openat(
            dir,
            partial.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode,
        );
        match created {
            Ok(file) => return Ok((partial, std::fs::File::from(file))),
            Err(Errno::EXIST) if attempt < 8 => attempt += 1,
            Err(e) => return Err(e.into()),
        }
    }
}
