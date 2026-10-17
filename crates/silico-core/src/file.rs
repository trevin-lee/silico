//! A document's file, read and written safely, for every client: the CLI's `fmt`, and the
//! app's save and MCP's as they arrive, write a document exactly alike. Only a regular file
//! is read (a pipe or a device could hang or never end), no more than a document may be;
//! a save never leaves a document part written or lost, writes through a symbolic link,
//! keeps a hard link's names together, and refuses a file this user may not write or one
//! changed since it was read. Every failure is [`ErrorCode::IoFailed`], with what to try.

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use crate::document::json::MAX_INPUT_LEN;
use crate::{Error, ErrorCode};

/// `file`'s bytes, if it's a regular file (a document is one: a pipe or a device, which
/// could hang or never end, isn't), at most one more than a document may be (the reader
/// refuses it then, saying so); or why not.
pub fn read(file: &Path) -> Result<Vec<u8>, Error> {
    opened_and_read(file).map(|(_, bytes)| bytes)
}

/// `file`, opened, and as much of it as [`read`] reads, the rest left to read.
pub fn opened_and_read(file: &Path) -> Result<(std::fs::File, Vec<u8>), Error> {
    // Opened without waiting (a pipe with no writer would block an open), then checked.
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file)
        .map_err(unreadable)?;
    let metadata = opened.metadata().map_err(unreadable)?;
    if !metadata.is_file() {
        return Err(unreadable("it isn't a regular file"));
    }
    let mut opened = opened;
    let mut bytes = Vec::new();
    (&mut opened)
        .take(MAX_INPUT_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    Ok((opened, bytes))
}

/// Where `file`'s rebuild cache is (`.silico-cache/`): beside the file it names, through a
/// link, so every name of a document, and every client, shares one.
pub fn cache_directory(file: &Path) -> PathBuf {
    let named = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned());
    directory(&named).join(".silico-cache")
}

/// The directory `file` is in (`.` for a bare name).
pub fn directory(file: &Path) -> &Path {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// A file that couldn't be read, `why`, and what to try.
fn unreadable(why: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::IoFailed, format!("couldn't be read: {why}"))
        .with_suggestion("check the file's name, and that it's a regular file this user may read")
}

/// A file that couldn't be written, `why`, and what to try.
/// `error`, advising to raise the limit if `cause` is a file-size limit's (EFBIG).
fn limited(error: Error, cause: &std::io::Error) -> Error {
    if cause.kind() == std::io::ErrorKind::FileTooLarge {
        error.with_suggestion(
            "a limit on a file's size stopped the write (a shell's `ulimit -f`): raise it, then save",
        )
    } else {
        error
    }
}

fn unwritable(why: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::IoFailed, format!("couldn't be written: {why}")).with_suggestion(
        "check that this user may write the file and its directory, and that the disk has room",
    )
}

/// Writes `bytes` over `file`, which held `before` when it was read: the file it names
/// (through a symbolic link, which stays), keeping its permissions. Whole, to a new file
/// beside it renamed over it, so a reader never sees a part of it and a write cut short
/// leaves the old file; or, if it has other names (hard links), which a rename would part
/// from it, in place, with the old text kept beside it until the new is on disk, and put
/// back if the write fails (a full disk), so a write cut short never loses the document
/// (killed mid-write, the old text is in the file beside it, `.NAME.fmt-PID`). Refused if
/// this process may not write it, or it changed since it was read.
pub fn save(file: &Path, before: &[u8], bytes: &[u8]) -> Result<(), Error> {
    let target = std::fs::canonicalize(file).map_err(unwritable)?;
    let metadata = std::fs::metadata(&target).map_err(unwritable)?;
    // Whether this process may write it, as the system says, whatever its mode bits say
    // (a file its owner can't write but its group can, another user's): opened to write,
    // not truncated.
    // Opened without waiting, and checked to be a regular file still: one swapped for a
    // pipe while it was rebuilt would block an open forever.
    let mut writable = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&target)
        .map_err(unwritable)?;
    if !writable.metadata().map_err(unwritable)?.is_file() {
        return Err(unwritable("it isn't a regular file now"));
    }
    // Changed while it was rebuilt (another editor saved it): left to them.
    if read(&target)? != before {
        return Err(Error::new(
            ErrorCode::IoFailed,
            "couldn't be written: it changed while it was rebuilt",
        )
        .with_suggestion(
            "it was saved by something else meanwhile: read it again (run `silico fmt` again), \
             then save",
        ));
    }
    if metadata.nlink() > 1 {
        return in_place(&target, &mut writable, &metadata, before, bytes);
    }
    let (partial, mut f) = beside(&target, &metadata)?;
    let written = f
        .write_all(bytes)
        .and_then(|()| f.set_permissions(metadata.permissions()))
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("couldn't write {}: {e}", partial.display()))
        .and_then(|()| {
            std::fs::rename(&partial, &target)
                .map_err(|e| format!("couldn't rename {} over it: {e}", partial.display()))
        });
    written.map_err(|why| {
        let _ = std::fs::remove_file(&partial);
        unwritable(why)
    })
}

/// `bytes` written over `target`, open in `file`, which held `before`: the old text kept
/// in a file beside it until the new is on disk, and put back if the write fails.
fn in_place(
    target: &Path,
    file: &mut std::fs::File,
    metadata: &std::fs::Metadata,
    before: &[u8],
    bytes: &[u8],
) -> Result<(), Error> {
    use std::io::{Seek as _, SeekFrom};
    let (kept, mut copy) = beside(target, metadata)?;
    // Grown to its length first, as the document is below: a size limit stops the copy
    // before any of it is written, so a copy left beside the file is all of the old text.
    if let Err(e) = copy
        .set_len(before.len() as u64)
        .and_then(|()| copy.write_all(before))
        .and_then(|()| copy.sync_all())
    {
        let _ = std::fs::remove_file(&kept);
        return Err(limited(
            unwritable(format!("couldn't keep its text in {}: {e}", kept.display())),
            &e,
        ));
    }
    let over = |file: &mut std::fs::File, text: &[u8]| {
        // Grown to its new length before a byte is written: a limit on a file's size (a
        // shell's `ulimit -f`) stops it here, with its text as it was, not part way.
        if text.len() > before.len() {
            file.set_len(text.len() as u64)?;
        }
        file.seek(SeekFrom::Start(0))?;
        file.write_all(text)?;
        file.set_len(text.len() as u64)?;
        file.sync_all()
    };
    match over(file, bytes) {
        Ok(()) => {
            let _ = std::fs::remove_file(&kept);
            Ok(())
        }
        // It fits back in the room it had.
        Err(e) => match over(file, before) {
            Ok(()) => {
                let _ = std::fs::remove_file(&kept);
                Err(limited(unwritable(format!("{e}; it's as it was")), &e))
            }
            Err(_) => Err(limited(
                unwritable(format!("{e}; its text before is in {}", kept.display())),
                &e,
            )),
        },
    }
}

/// How many names [`beside`] tries.
const TRIES: u32 = 64;

/// A new file beside `target` (`.NAME.fmt-PID`, or `-1`, `-2`, ... after it if one is
/// left from a run killed with the same process number), no more open to others than
/// `target` is.
fn beside(target: &Path, metadata: &std::fs::Metadata) -> Result<(PathBuf, std::fs::File), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = directory(target);
    let name = target
        .file_name()
        .ok_or_else(|| unwritable("it has no file name"))?;
    for n in 0..TRIES {
        let mut partial_name = OsString::from(".");
        partial_name.push(name);
        partial_name.push(format!(".fmt-{}", std::process::id()));
        if n > 0 {
            partial_name.push(format!("-{n}"));
        }
        let partial = dir.join(partial_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(metadata.permissions().mode() & 0o777)
            .open(&partial)
        {
            Ok(f) => return Ok((partial, f)),
            // Left by a run killed with this process's number: the next name.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(unwritable(format!(
                    "couldn't create a file in {}: {e}",
                    dir.display()
                )));
            }
        }
    }
    Err(unwritable(format!(
        "every name for a file beside it is taken ({}/.{}.fmt-{}, and -1 to -{}): remove those left by runs that were killed",
        dir.display(),
        name.to_string_lossy(),
        std::process::id(),
        TRIES - 1
    )))
}
