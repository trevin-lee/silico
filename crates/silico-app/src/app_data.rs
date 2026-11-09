//! The app's own files in the platform's app-data folder ([`folder`]): the recent documents
//! (`crate::recent`) and the side panes' widths (`crate::pane_widths`), each kept by a thread
//! of its own. What they share is here: where the folder is, a capped read that never hangs
//! on what isn't a regular file ([`read_capped`]), and a write a reader never sees half of
//! ([`write`]).

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    if !std::fs::metadata(file).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .ok()?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
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

/// The number of the next file written beside a kept one: with the process's ID, a name no
/// other write, in this instance of the app or another, uses at once.
static WRITES: AtomicU64 = AtomicU64::new(0);

/// `text` written to `file`, its folder made if it isn't there: to a file of its own beside
/// it first, flushed to the disk, then moved over it, so a reader never sees half of it;
/// that file removed if the write fails.
pub(crate) fn write(file: &Path, text: &str) -> std::io::Result<()> {
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
        new.write_all(text.as_bytes())?;
        new.sync_all()?;
        std::fs::rename(&beside, file)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    written
}
