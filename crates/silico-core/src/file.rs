//! A document's file, read and written safely, for every client: the CLI's `fmt`, and the
//! app's save and MCP's as they arrive, write a document exactly alike. Only a regular file
//! is read (a pipe or a device could hang or never end), no more than a document may be;
//! a save never leaves a document part written or lost, writes through a symbolic link,
//! keeps a hard link's names together, and refuses a file this user may not write or one
//! changed since it was read. Every failure is [`ErrorCode::IoFailed`], with what to try,
//! but a file changed since it was read, which is [`ErrorCode::Conflict`] (decision 16 of
//! docs/design/m4.1-command-api.md): what to do about it is to read it again, not to mend
//! the disk or a permission.

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
pub(crate) fn unreadable(why: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::IoFailed, format!("couldn't be read: {why}"))
        .with_suggestion("check the file's name, and that it's a regular file this user may read")
}

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

/// A file that couldn't be written, `why`, and what to try.
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
/// this process may not write it, or, as a conflict, if it changed since it was read.
pub fn save(file: &Path, before: &[u8], bytes: &[u8]) -> Result<(), Error> {
    save_over(file, before, bytes).map_err(|unsaved| unsaved.error)
}

/// A save that failed ([`save_over`]): why, and, if its own write in place left the file
/// part written and its old text couldn't be put back, the file beside it that holds that
/// text, so whoever saves next can say so rather than take the file for one changed by
/// someone else.
#[derive(Debug)]
pub struct Unsaved {
    pub error: Error,
    pub old_text_in: Option<PathBuf>,
}

impl From<Error> for Unsaved {
    fn from(error: Error) -> Self {
        Unsaved {
            error,
            old_text_in: None,
        }
    }
}

/// [`save`], saying, when it fails, whether its own write left the file part written
/// ([`Unsaved`]).
pub fn save_over(file: &Path, before: &[u8], bytes: &[u8]) -> Result<(), Unsaved> {
    let _span = tracing::info_span!("save", bytes = bytes.len()).entered();
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
        return Err(Unsaved::from(unwritable("it isn't a regular file now")));
    }
    // Changed while it was rebuilt (another editor saved it): left to them.
    if read(&target)? != before {
        return Err(Unsaved::from(
            Error::new(
                ErrorCode::Conflict,
                "couldn't be written: it changed while it was rebuilt",
            )
            .with_suggestion(
                "it was saved by something else meanwhile: read it again (run `silico fmt` again), \
             then save",
            ),
        ));
    }
    if metadata.nlink() > 1 {
        return in_place(&target, &mut writable, &metadata, before, bytes);
    }
    let (partial, mut f) = beside(&target, &metadata)?;
    // Grown to its length first, as an in-place write is: a size limit stops it before a
    // byte is written, so nothing beside the file is part of the new text.
    let written = f
        .set_len(bytes.len() as u64)
        .and_then(|()| f.write_all(bytes))
        .and_then(|()| f.set_permissions(metadata.permissions()))
        .and_then(|()| synced(&f, "the new text beside it"))
        .map_err(|e| {
            let why = format!("couldn't write {}: {e}", shown_path(&partial));
            limited(unwritable(why), &e)
        })
        .and_then(|()| {
            std::fs::rename(&partial, &target).map_err(|e| {
                unwritable(format!(
                    "couldn't rename {} over it: {e}",
                    shown_path(&partial)
                ))
            })
        });
    written
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&partial);
        })
        .map_err(Unsaved::from)
}

/// `bytes` written over `target`, open in `file`, which held `before`: the old text kept
/// in a file beside it until the new is on disk, and put back if the write fails.
fn in_place(
    target: &Path,
    file: &mut std::fs::File,
    metadata: &std::fs::Metadata,
    before: &[u8],
    bytes: &[u8],
) -> Result<(), Unsaved> {
    use std::io::{Seek as _, SeekFrom};
    let (kept, mut copy) = beside(target, metadata)?;
    // Grown to its length first, as the document is below: a size limit stops the copy
    // before any of it is written, so a copy left beside the file is all of the old text,
    // or nothing (killed by the limit as it grew), never a part of it.
    if let Err(e) = copy
        .set_len(before.len() as u64)
        .and_then(|()| copy.write_all(before))
        .and_then(|()| synced(&copy, "its old text kept beside it"))
    {
        let _ = std::fs::remove_file(&kept);
        return Err(Unsaved::from(limited(
            unwritable(format!(
                "couldn't keep its text in {}: {e}",
                shown_path(&kept)
            )),
            &e,
        )));
    }
    let over = |file: &mut std::fs::File, text: &[u8], what: &str| {
        // Grown to its new length before a byte is written: a limit on a file's size (a
        // shell's `ulimit -f`) stops it here, with its text as it was, not part way.
        if text.len() > before.len() {
            file.set_len(text.len() as u64)?;
        }
        file.seek(SeekFrom::Start(0))?;
        file.write_all(text)?;
        file.set_len(text.len() as u64)?;
        synced(file, what)
    };
    match over(file, bytes, "its new text, in place") {
        Ok(()) => {
            let _ = std::fs::remove_file(&kept);
            Ok(())
        }
        // It fits back in the room it had.
        Err(e) => match over(file, before, "its old text, put back") {
            Ok(()) => {
                let _ = std::fs::remove_file(&kept);
                Err(Unsaved::from(limited(
                    unwritable(format!("{e}; it's as it was")),
                    &e,
                )))
            }
            Err(_) => Err(Unsaved {
                error: limited(
                    unwritable(format!("{e}; its text before is in {}", shown_path(&kept))),
                    &e,
                ),
                old_text_in: Some(kept),
            }),
        },
    }
}

/// `file` on disk, as `sync_all` puts it, said in the log before and after with how long it
/// took (debug): a disk that stalls a save shows as the line it stopped at.
fn synced(file: &std::fs::File, what: &str) -> std::io::Result<()> {
    tracing::debug!("syncing {what}");
    let start = std::time::Instant::now();
    let synced = file.sync_all();
    let sync_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    match &synced {
        Ok(()) => tracing::debug!(sync_ms, "synced {what}"),
        Err(e) => tracing::debug!(sync_ms, error = %e, "couldn't sync {what}"),
    }
    synced
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
                    shown_path(dir)
                )));
            }
        }
    }
    let first = dir.join(format!(
        ".{}.fmt-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    Err(unwritable(format!(
        "every name for a file beside it is taken ({}, and -1 to -{}): remove those left by runs that were killed",
        shown_path(&first),
        TRIES - 1
    )))
}

/// A path as an error says it: quoted, with control and invisible characters escaped, as an
/// outline quotes a document's text, so a file's name can't act on the terminal it's shown
/// in (found by the milestone's robustness review).
fn shown_path(path: &Path) -> String {
    crate::command::quoted(&path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_size_limit_gets_the_limits_advice() {
        // Found by the review: an error on a full disk told to raise a limit that isn't the
        // cause.
        let full = std::io::Error::from_raw_os_error(libc::ENOSPC);
        assert!(
            !limited(unwritable("x"), &full)
                .to_string()
                .contains("ulimit")
        );
        let big = std::io::Error::from_raw_os_error(libc::EFBIG);
        assert!(
            limited(unwritable("x"), &big)
                .to_string()
                .contains("ulimit -f")
        );
    }
}

/// The rules a file a command writes besides the document is held to (decision 35:
/// `render`'s `to`, `export`'s `path`; docs/design/m4.1-command-api.md, *Files a command
/// writes*).
#[derive(Debug, Clone, Copy)]
pub struct Output<'a> {
    /// The request's field the path came in (`to`, `path`), which every refusal names.
    pub field: &'a str,
    /// The format's extension, without its dot (`png`, `3mf`).
    pub extension: &'a str,
    /// Whether a file already at the path may be replaced.
    pub replace: bool,
    /// The workspace the path must be inside, never through a link out of it, if the
    /// sessions have one (decision 26).
    pub workspace: Option<&'a Path>,
    /// The documents the sessions hold, which it may never be.
    pub held: &'a [PathBuf],
}

/// Writes `bytes` to `path` as `rules` allow, and gives where it wrote (the path through
/// any links, canonical): refused, naming the field, if the path is a document (ends in
/// `.silico`, is one the sessions hold, or lies in a `.silico-cache/` directory), lacks the
/// format's extension, lies outside the workspace or reaches out of it through a link, has
/// no directory, or names a file already there that `replace` doesn't allow replacing (one
/// replaced keeps its permissions). Whole, to a new file beside it renamed into place, as
/// [`save`] writes, so a write cut short leaves what was there.
pub fn write_output(path: &Path, bytes: &[u8], rules: &Output) -> Result<PathBuf, Error> {
    let (target, replaced) = output_target(path, rules)?;
    write_beside(&target, replaced.as_ref(), bytes)?;
    Ok(target)
}

/// Where [`write_output`] would write `path`, refused as it refuses, writing nothing: for a
/// command to refuse a path before the work whose result it writes (a render). The path is
/// checked again as it's written, as what's there may change between.
pub fn check_output(path: &Path, rules: &Output) -> Result<PathBuf, Error> {
    Ok(output_target(path, rules)?.0)
}

/// The path `path` writes to, through any links, and the file it would replace, if one; or
/// why `rules` refuse it.
fn output_target(
    given: &Path,
    rules: &Output,
) -> Result<(PathBuf, Option<std::fs::Metadata>), Error> {
    // Relative to the workspace when there's one (M4.1 section 2), as given in messages.
    let path = given;
    let joined;
    let path = match rules.workspace {
        Some(root) if given.is_relative() => {
            joined = root.join(given);
            joined.as_path()
        }
        _ => path,
    };
    let refuse = |why: String, suggestion: &str| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("{} {}: {why}", rules.field, shown_path(given)),
        )
        .with_parameter(rules.field)
        .with_suggestion(suggestion)
    };
    let has = |p: &Path, ext: &str| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
    };
    if has(path, "silico") {
        return Err(refuse(
            "is a document's path: only commit and format write a document".to_owned(),
            "write it to a path that isn't a document's",
        ));
    }
    if !has(path, rules.extension) {
        return Err(refuse(
            format!("doesn't end in .{}", rules.extension),
            &format!("give a path ending in .{}", rules.extension),
        ));
    }
    let name = path.file_name().ok_or_else(|| {
        refuse(
            "has no file name".to_owned(),
            "give the path of a file to write",
        )
    })?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let dir = std::fs::canonicalize(parent).map_err(|e| {
        refuse(
            format!("its directory can't be reached: {e}"),
            "give a path in a directory that exists",
        )
    })?;
    // Where it would be written: through a link already at the path, to what it names.
    let existing = std::fs::symlink_metadata(dir.join(name)).ok();
    let target = match &existing {
        Some(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(dir.join(name))
            .map_err(|e| {
                refuse(
                    format!("is a link to nothing that can be reached: {e}"),
                    "give a path that isn't a broken link",
                )
            })?,
        _ => dir.join(name),
    };
    if target
        .components()
        .any(|c| c.as_os_str() == ".silico-cache")
    {
        return Err(refuse(
            "lies in a .silico-cache directory, which holds a document's cache".to_owned(),
            "write it outside the .silico-cache directory",
        ));
    }
    if has(&target, "silico") {
        return Err(refuse(
            "is a link to a document".to_owned(),
            "write it to a path that isn't a document's",
        ));
    }
    let held: Vec<PathBuf> = rules
        .held
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();
    if held.contains(&target) {
        return Err(refuse(
            "is a document a session holds".to_owned(),
            "write it to a path that isn't a document's",
        ));
    }
    if let Some(root) = rules.workspace {
        let root = std::fs::canonicalize(root).map_err(|e| {
            refuse(
                format!("the workspace can't be reached: {e}"),
                "start Silico in a workspace that exists",
            )
        })?;
        if !target.starts_with(&root) {
            return Err(refuse(
                format!("is outside the workspace {}", shown_path(&root)),
                "write it inside the workspace, not through a link out of it",
            ));
        }
    }
    let replaced = match std::fs::metadata(&target) {
        Ok(meta) if !meta.is_file() => {
            return Err(refuse(
                "is there, and isn't a file".to_owned(),
                "give the path of a file to write",
            ));
        }
        Ok(_) if !rules.replace => {
            return Err(refuse(
                "is there already".to_owned(),
                "give another path, or replace: true to replace it",
            ));
        }
        Ok(meta) => Some(meta),
        Err(_) => None,
    };
    Ok((target, replaced))
}

/// `bytes` written to `target` through a new file beside it renamed into place, with the
/// permissions of the file it replaces, if one (or the new file's own).
fn write_beside(
    target: &Path,
    replaced: Option<&std::fs::Metadata>,
    bytes: &[u8],
) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = replaced.map_or(0o644, |m| m.permissions().mode() & 0o777);
    let dir = directory(target);
    let name = target
        .file_name()
        .ok_or_else(|| unwritable("it has no file name"))?;
    let mut created = None;
    for n in 0..TRIES {
        // Beside it, named for it, unless that would pass a file name's 255 bytes.
        let suffix = format!(".part-{}", std::process::id());
        let mut partial_name = OsString::from(".");
        if name.len() + suffix.len() + 8 <= 255 {
            partial_name.push(name);
        } else {
            partial_name.push("silico-output");
        }
        partial_name.push(suffix);
        if n > 0 {
            partial_name.push(format!("-{n}"));
        }
        let partial = dir.join(partial_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&partial)
        {
            Ok(f) => {
                created = Some((partial, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(unwritable(format!(
                    "couldn't create a file in {}: {e}",
                    shown_path(dir)
                )));
            }
        }
    }
    let (partial, mut f) = created.ok_or_else(|| {
        unwritable(format!(
            "every name for a file beside it is taken: remove the .part files in {}",
            shown_path(dir)
        ))
    })?;
    let written = f
        .set_len(bytes.len() as u64)
        .and_then(|()| f.write_all(bytes))
        .and_then(|()| match replaced {
            Some(m) => f.set_permissions(m.permissions()),
            None => Ok(()),
        })
        .and_then(|()| synced(&f, "the new file"))
        .map_err(|e| {
            limited(
                unwritable(format!("couldn't write {}: {e}", shown_path(&partial))),
                &e,
            )
        })
        .and_then(|()| {
            std::fs::rename(&partial, target).map_err(|e| {
                unwritable(format!(
                    "couldn't rename {} into place: {e}",
                    shown_path(&partial)
                ))
            })
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written
}
