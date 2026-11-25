//! The app's own files in the platform's app-data folder ([`folder`]): the recent documents
//! (`crate::recent`), the side panes' widths (`crate::pane_widths`), and the start page's
//! thumbnails (`crate::thumbs`), each kept by a thread of its own. What they share is here:
//! where the folder is, capped reads that never hang on what isn't a regular file
//! ([`read_capped`], [`read_up_to`]), a write a reader never sees half of ([`write`],
//! [`write_bytes`]), and the rule for what such a write left if it never finished
//! ([`left_by_a_write`], [`remove_left_over`]).

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// What reads a kept file the first time: its bytes, or none if it can't be read (a test's,
/// to hold the read).
pub type Read = Box<dyn FnOnce(&Path) -> Option<Vec<u8>> + Send>;

/// The platform's app-data folder for Silico: `~/Library/Application Support/Silico` on
/// macOS, `$XDG_DATA_HOME/silico` or `~/.local/share/silico` on Linux; none if the platform
/// names no home folder.
pub(crate) fn folder() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Silico").map(|dirs| dirs.data_dir().to_path_buf())
}

/// `file`'s first `max` bytes, if it's a regular file that can be read (a pipe, which could
/// hang the read, isn't); if there's more, the line the cap cuts is dropped too.
pub(crate) fn read_capped(file: &Path, max: usize) -> Option<Vec<u8>> {
    let mut bytes = read_up_to(file, max)?;
    if bytes.len() > max {
        bytes.truncate(max);
        let whole = bytes
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        bytes.truncate(whole);
    }
    Some(bytes)
}

/// `file`'s first `max` bytes and one more, if there's more, so the caller can tell: none if
/// it isn't a regular file that can be read (a pipe, which could hang the read, isn't).
pub(crate) fn read_up_to(file: &Path, max: usize) -> Option<Vec<u8>> {
    if !std::fs::metadata(file).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .ok()?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// The number of the next file written beside a kept one: with the process's ID, a name no
/// other write, in this instance of the app or another, uses at once.
static WRITES: AtomicU64 = AtomicU64::new(0);

/// `text` written to `file`, as [`write_bytes`] writes it.
pub(crate) fn write(file: &Path, text: &str) -> std::io::Result<()> {
    write_bytes(file, text.as_bytes())
}

/// `bytes` written to `file`, its folder made if it isn't there: to a file of its own beside
/// it first, flushed to the disk, then moved over it, so a reader never sees half of it;
/// that file removed if the write fails.
pub(crate) fn write_bytes(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut beside = file.as_os_str().to_owned();
    beside.push(format!(
        ".{}.{}.new",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let beside = PathBuf::from(beside);
    let written = (|| {
        let mut new = std::fs::File::create(&beside)?;
        new.write_all(bytes)?;
        new.sync_all()?;
        std::fs::rename(&beside, file)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    written
}

/// How old a file left by a write that never finished (`<name>.<pid>.<n>.new`, a quit or a
/// kill mid-write) is before it's removed: far longer than any write takes, so a write under
/// way, this instance's or another's, is never taken for one.
pub const LEFT_OVER_AGE: Duration = Duration::from_secs(60 * 60);

/// Whether a file named `name` is one a write of a kept file left ([`write_bytes`]'s
/// `<kept>.<pid>.<n>.new`), `kept` saying whether `<kept>` is the name of a kept file;
/// nothing else is.
pub(crate) fn left_by_a_write(name: &str, kept: impl Fn(&str) -> bool) -> bool {
    let Some(rest) = name.strip_suffix(".new") else {
        return false;
    };
    let mut parts = rest.rsplitn(3, '.');
    let (Some(n), Some(pid), Some(written)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    number(n) && number(pid) && kept(written)
}

/// Whether a file modified at `modified` is older than [`LEFT_OVER_AGE`] at `now`: never one
/// modified after `now` (a clock set back).
pub(crate) fn left_over_long(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age > LEFT_OVER_AGE)
}

/// Each file a write of `file` left beside it ([`left_by_a_write`]) and older than
/// [`LEFT_OVER_AGE`] at `now` removed, as a kept file's thread starts; any other file, and a
/// younger one, which may be another instance's write under way, left alone.
pub(crate) fn remove_left_over(file: &Path, now: SystemTime) {
    let (Some(folder), Some(kept)) = (file.parent(), file.file_name().and_then(|n| n.to_str()))
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !left_by_a_write(name, |written| written == kept) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if left_over_long(modified, now) {
            let left = folder.join(name);
            if let Err(e) = std::fs::remove_file(&left) {
                tracing::info!("{} couldn't be removed: {e}", left.display());
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_writes_left_over_is_its_kept_files_name_a_pid_a_number_and_new() {
        let kept = |written: &str| written == "recent-documents";
        assert!(left_by_a_write("recent-documents.123.4.new", kept));
        assert!(left_by_a_write("recent-documents.1.0.new", kept));
        // Not a write's: no number, a number of letters, another file's, no `.new`.
        for name in [
            "recent-documents",
            "recent-documents.new",
            "recent-documents.123.new",
            "recent-documents..4.new",
            "recent-documents.123..new",
            "recent-documents.x.4.new",
            "recent-documents.123.4x.new",
            "pane-widths.123.4.new",
            "recent-documents.123.4.old",
            "recent-documents.123.4.new.new",
        ] {
            assert!(!left_by_a_write(name, kept), "{name}");
        }
    }

    #[test]
    fn a_kept_files_left_overs_older_than_an_hour_are_removed_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("recent-documents");
        let names = [
            "recent-documents",
            "recent-documents.123.4.new",
            "recent-documents.123.5.new",
            "recent-documents.x.4.new",
            "pane-widths.123.4.new",
            "notes.txt",
        ];
        for name in names {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let modified = |name: &str| {
            std::fs::metadata(dir.path().join(name))
                .unwrap()
                .modified()
                .unwrap()
        };
        let listed = || {
            let mut listed: Vec<String> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            listed.sort();
            listed
        };
        // An hour on, to the second, from the newest: nothing yet (older than, not as old as).
        let newest = names.iter().map(|name| modified(name)).max().unwrap();
        remove_left_over(&file, newest + Duration::from_secs(60 * 60));
        let mut all: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        all.sort();
        assert_eq!(listed(), all);
        // Past it: the two the kept file's writes left, alone.
        remove_left_over(&file, newest + Duration::from_secs(60 * 60 + 1));
        assert_eq!(
            listed(),
            [
                "notes.txt",
                "pane-widths.123.4.new",
                "recent-documents",
                "recent-documents.x.4.new"
            ]
        );
        // A clock set back: nothing.
        std::fs::write(dir.path().join("recent-documents.9.9.new"), b"x").unwrap();
        remove_left_over(&file, SystemTime::UNIX_EPOCH);
        assert!(listed().contains(&"recent-documents.9.9.new".to_owned()));
        // No folder: nothing, and no panic.
        remove_left_over(&dir.path().join("gone").join("recent-documents"), newest);
        remove_left_over(Path::new("/"), newest);
    }

    /// `name` in `folder`, written as a write that never finished left it, modified two
    /// hours ago.
    pub(crate) fn left_two_hours_ago(folder: &Path, name: &str) {
        let left = folder.join(name);
        std::fs::write(&left, b"half").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&left)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(2 * 60 * 60))
            .unwrap();
    }
}
