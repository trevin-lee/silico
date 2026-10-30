//! A document's file, read and written safely, for every client: the CLI's `fmt`, and the
//! app's save and MCP's as they arrive, write a document exactly alike. Only a regular file
//! is read (a pipe or a device could hang or never end), no more than a document may be;
//! a save never leaves a document part written or lost, writes through a symbolic link,
//! keeps a hard link's names together, and refuses a file this user may not write or one
//! changed since it was read. Every failure is [`ErrorCode::IoFailed`], with what to try,
//! but a file changed since it was read, which is [`ErrorCode::Conflict`] (decision 16 of
//! docs/design/m4.1-command-api.md): what to do about it is to read it again, not to mend
//! the disk or a permission.
//!
//! And the workspace ([`Workspace`], decision 26), the directory `silico serve` is started
//! in, which every path a client gives must lie in: [`resolve`] finds where a path leads
//! without ever looking up a name outside the workspace, so nothing there is told apart by
//! whether it exists; [`save_within`] saves a session's file only while it's still inside;
//! and [`write_output`] writes a command's file (a render's `to`) only inside it.

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};

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
    opened_and_read_io(file).map_err(unreadable)
}

/// [`opened_and_read`], its failure the system's own, its kind kept (no file at the path is
/// `NotFound`; a file that isn't a regular one, `InvalidInput`): for a caller that tells
/// them apart, as a disk comparison tells a file gone from one it can't read.
pub(crate) fn opened_and_read_io(file: &Path) -> std::io::Result<(std::fs::File, Vec<u8>)> {
    // Opened without waiting (a pipe with no writer would block an open), then checked.
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file)?;
    let metadata = opened.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "it isn't a regular file",
        ));
    }
    let mut opened = opened;
    let mut bytes = Vec::new();
    (&mut opened)
        .take(MAX_INPUT_LEN as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok((opened, bytes))
}

/// Where `file`'s rebuild cache is (`.silico-cache/`): beside the file it names, through a
/// link, so every name of a document, and every client, shares one.
pub fn cache_directory(file: &Path) -> PathBuf {
    let named = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned());
    directory(&named).join(".silico-cache")
}

/// The directory every path a client gives must lie in, through its links (decision 26 of
/// docs/design/m4.1-command-api.md): what `silico serve` is started in. Its root is
/// canonicalized once, when it's made, and every check is of a path walked through its
/// links, never into a directory outside the root, against that root, component by
/// component, so a sibling whose name starts with the root's (`work2` beside `work`) is
/// outside it. One implementation of the rule ([`resolve`]), which every place a path
/// resolves holds to: `open` (a session's file, and the directory a new one is made in),
/// each re-read and save of a session's file, its directory's `.silico-cache/`, and a file
/// a command writes ([`Output`]). Without one (the CLI), paths resolve as the system
/// resolves them, against the process's working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
    /// The root as the system names an open handle of it, taken once, as the workspace is
    /// made, from a handle opened on the canonical root and checked to be it: a spelling
    /// the system may give a handle's path in that `realpath` doesn't (macOS names
    /// `/System/Volumes/Data/Users/...` as `/Users/...`, its firmlink). None if it can't
    /// be had; the canonical root then stands alone.
    as_held: Option<PathBuf>,
}

/// How many links [`Workspace::walk`] follows through a path before it decides by the
/// path's form: Linux's and macOS's own limit on links followed in one lookup.
const LINKS_FOLLOWED: u32 = 40;

/// The longest path, in bytes, [`Workspace::walk`] walks on the file system; a longer
/// one it decides by its form: Linux's `PATH_MAX`, the longest path it resolves (macOS's is
/// shorter). So a path of millions of names costs no more than reading it once.
const WALKED_PATH_BYTES: usize = 4096;

/// Where a path's walk ends ([`Workspace::walk`]).
#[derive(Debug, PartialEq, Eq)]
enum Walked {
    Out,
    In,
    /// In the workspace, at a file with more of the path to go.
    ThroughAFile,
}

/// One step of a path walked as the system resolves it ([`Workspace::walk`]).
enum Step {
    /// Start again from here: the file system's top (and on Windows, the drive).
    From(PathBuf),
    /// The directory above.
    Up,
    /// The entry by this name.
    Name(OsString),
}

impl Step {
    /// `path`'s steps, in order, its `.`s none.
    fn of(path: &Path) -> Vec<Step> {
        let mut steps = Vec::new();
        let mut top = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) => top.push(component),
                Component::RootDir => {
                    top.push(component);
                    steps.push(Step::From(std::mem::take(&mut top)));
                }
                Component::CurDir => {}
                Component::ParentDir => steps.push(Step::Up),
                Component::Normal(name) => steps.push(Step::Name(name.to_owned())),
            }
        }
        steps
    }
}

/// `canonical`, a directory's canonical path, as the system names a handle opened on it
/// ([`Workspace::as_held`]): the handle opened with no link followed at its name, and taken
/// only if it is the directory `canonical` names now (the same device and inode), so a link
/// put there meanwhile gives none, never its target's spelling.
fn held_spelling(canonical: &Path) -> Option<PathBuf> {
    use rustix::fs::CWD;
    let handle = crate::cache::open_dir(CWD, canonical).ok()?;
    let opened = rustix::fs::fstat(&handle).ok()?;
    let named = rustix::fs::statat(CWD, canonical, rustix::fs::AtFlags::SYMLINK_NOFOLLOW).ok()?;
    if (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino) {
        return None;
    }
    crate::cache::path_of(&handle).ok()
}

impl Workspace {
    /// The workspace at `root`, through its links: refused if it isn't a directory that can
    /// be reached.
    pub fn new(root: &Path) -> Result<Self, Error> {
        let canonical = std::fs::canonicalize(root).map_err(|e| {
            Error::new(
                ErrorCode::IoFailed,
                format!("the workspace {} can't be reached: {e}", shown_path(root)),
            )
            .with_suggestion("start Silico in a workspace that exists")
        })?;
        if !canonical.is_dir() {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!("the workspace {} isn't a directory", shown_path(root)),
            )
            .with_suggestion("start Silico in a directory"));
        }
        let as_held = held_spelling(&canonical);
        Ok(Workspace {
            root: canonical,
            as_held,
        })
    }

    /// Its root, through its links.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `resolved`, a path through its links, lies in the workspace: its root or
    /// below it.
    pub fn holds(&self, resolved: &Path) -> bool {
        resolved.starts_with(&self.root)
            || self
                .as_held
                .as_ref()
                .is_some_and(|root| resolved.starts_with(root))
    }

    /// Whether `path`, a path from the file system's top, leads out of the workspace, decided
    /// as the system would walk it, a name at a time, through its links, but never looking
    /// a name up in a directory outside the workspace: the walk only looks in the root, a
    /// directory below it, or one of the root's ancestors (the directories above it, from the
    /// file system's top), and of an ancestor's names it only goes on through the next on
    /// the way down to the root (by any spelling the file system takes for it: another case,
    /// another Unicode form) or a link. Any other name in an ancestor, whether or not
    /// anything is there by it, leads out, nothing below it looked at: so nothing outside the
    /// workspace is told apart by whether it exists (`../secret/../work/x` and
    /// `../nowhere/../work/x` alike), and no lookup outside it can wait on a slow or a
    /// mounted-on-demand directory. A path that stops inside the workspace (a name missing
    /// there, or one below a file) leads nowhere out of it, as the system goes no further:
    /// what's wrong with it is the system's to say. So is a path to the root, or one of its
    /// ancestors on the way, gone (the workspace removed since it was made): it's
    /// unreachable, never outside. A path longer than the system resolves
    /// ([`WALKED_PATH_BYTES`]), or through more links than it follows ([`LINKS_FOLLOWED`]),
    /// is decided by its form alone ([`Workspace::leaves`]), so the walk is bounded.
    #[cfg(test)]
    fn leads_out(&self, path: &Path) -> bool {
        self.walk(path) == Walked::Out
    }

    /// Where `path`'s walk ends: out of the workspace, in it, or
    /// in it at a file with more of the path to go, where the system stops (`ENOTDIR`).
    /// That last isn't left to `canonicalize`: macOS's `realpath` takes a `..` after a file
    /// by its text and goes on, outside the workspace, where glibc's stops (review).
    fn walk(&self, path: &Path) -> Walked {
        let out = |out: bool| if out { Walked::Out } else { Walked::In };
        if path.as_os_str().len() > WALKED_PATH_BYTES {
            return out(self.leaves(path));
        }
        // The path walked from the top a name at a time, as the system resolves it: the
        // steps still to take (the next last), and where the walk is, through every link, so
        // a `..` is the directory above it. It's only ever the root, below it, or one of its
        // ancestors, spelled as the root is. A link's target takes its place, and costs one
        // of the budget; only a link does.
        let mut steps = Step::of(path);
        steps.reverse();
        let mut at = PathBuf::new();
        let mut links = LINKS_FOLLOWED;
        while let Some(step) = steps.pop() {
            let name = match step {
                Step::From(top) => {
                    at = top;
                    continue;
                }
                Step::Up => {
                    at.pop();
                    continue;
                }
                Step::Name(name) => name,
            };
            let next = at.join(&name);
            // Of an ancestor's names, the next on the way to the root; none in the workspace.
            let way = if self.holds(&at) {
                None
            } else {
                match self
                    .root
                    .strip_prefix(&at)
                    .map(|rest| rest.components().next())
                {
                    Ok(Some(Component::Normal(way))) => Some(at.join(way)),
                    // Not the root's ancestor (a path not from the top): nowhere to look.
                    _ => return Walked::Out,
                }
            };
            let meta = std::fs::symlink_metadata(&next);
            if let Ok(meta) = &meta
                && meta.file_type().is_symlink()
            {
                let Some(left) = links.checked_sub(1) else {
                    return out(self.leaves(path));
                };
                links = left;
                let Ok(target) = std::fs::read_link(&next) else {
                    // Gone since it was looked at: the system stops here.
                    return out(!self.holds(&at));
                };
                steps.extend(Step::of(&target).into_iter().rev());
                continue;
            }
            match (meta, way) {
                // In the workspace: a directory to go on in; a file with more to go, where
                // the system stops, not a directory; a file at the end, or nothing, in it.
                (Ok(meta), None) if meta.is_dir() => at = next,
                (Ok(_), None) if !steps.is_empty() => return Walked::ThroughAFile,
                (_, None) => return Walked::In,
                // The way to the root, by its own spelling or another the file system takes
                // for the same directory: on, spelled as the root is.
                (Ok(meta), Some(way))
                    if meta.is_dir() && (next == way || same_file(&meta, &way)) =>
                {
                    at = way;
                }
                // The way to the root, a file now (the workspace removed and a file put in
                // its place, or in an ancestor's) with more of the path to go: the system
                // stops there, as it does at a file in the workspace, never left to
                // `canonicalize` (macOS's would take a `..` after it by its text). Only on
                // the way: of an ancestor's other names, a file isn't told from nothing.
                (Ok(meta), Some(way)) if next == way && !meta.is_dir() && !steps.is_empty() => {
                    return Walked::ThroughAFile;
                }
                // The way to the root, not a directory or not there: the workspace gone, as
                // the system will say.
                (_, Some(way)) if next == way => return Walked::In,
                // Any other name of an ancestor: out, whatever is there.
                (_, Some(_)) => return Walked::Out,
            }
        }
        out(!self.holds(&at))
    }

    /// Whether `path`, its `.`s and `..`s taken as written, links not followed, leaves the
    /// workspace: what [`Workspace::walk`] decides when walking the path would cost
    /// more than the system itself spends on one. Nothing on the file system decides it.
    fn leaves(&self, path: &Path) -> bool {
        let mut normal = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normal.pop();
                }
                other => normal.push(other),
            }
        }
        !self.holds(&normal)
    }

    /// The refusal of `given`, the request's `field`, a path outside the workspace or one
    /// that reaches out of it through a link (M4.1's `open` example).
    pub fn outside(given: &Path, field: &str) -> Error {
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} is outside the workspace this server was started in",
                shown_path(given)
            ),
        )
        .with_parameter(field)
        .with_suggestion("give a path inside the workspace; a link out of it is refused too")
    }

    /// The refusal of a command whose session's file, `given` as `open` was given it, now
    /// leads out of the workspace (a link put on its path since): `invalid_input`, naming
    /// `path`, as `open` refuses one, never `io_failed`.
    pub fn outside_now(given: &Path) -> Error {
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} is outside the workspace this server was started in",
                shown_path(given)
            ),
        )
        .with_parameter("path")
        .with_cause(
            "it was inside the workspace when it was opened, and leads out of it now, through a link put on its path",
        )
        .with_suggestion(
            "put the document back inside the workspace, or close the session and open one inside it",
        )
    }
}

/// Whether `meta` is of the file at `path`, links not followed: the same device and inode,
/// however each is spelled.
fn same_file(meta: &std::fs::Metadata, path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|other| other.dev() == meta.dev() && other.ino() == meta.ino())
}

/// Why a path doesn't resolve inside a workspace ([`resolve`]).
#[derive(Debug)]
pub enum Unresolved {
    /// It leads out of the workspace, through its links or its `..`s, whether or not
    /// anything is there.
    Outside,
    /// It leads nowhere the system can follow: the system's error.
    Unreachable(std::io::Error),
}

/// Where `path` leads through its links, relative to `workspace`'s root if it's relative and
/// there is one, as the system resolves it: refused if it leads out of the workspace
/// ([`Unresolved::Outside`]), or, inside it, if it leads nowhere ([`Unresolved::Unreachable`]).
/// Without a workspace, exactly `std::fs::canonicalize(path)`.
pub fn resolve(workspace: Option<&Workspace>, path: &Path) -> Result<PathBuf, Unresolved> {
    let Some(workspace) = workspace else {
        return std::fs::canonicalize(path).map_err(Unresolved::Unreachable);
    };
    let joined = workspace.root.join(path);
    // Walked first, never looking outside the workspace, so only a path that stays in it
    // (or stops in it) is ever resolved by the system.
    match workspace.walk(&joined) {
        Walked::Out => return Err(Unresolved::Outside),
        Walked::ThroughAFile => {
            return Err(Unresolved::Unreachable(std::io::Error::from_raw_os_error(
                libc::ENOTDIR,
            )));
        }
        Walked::In => {}
    }
    // Longer than the system resolves: refused as the system refuses it, never walked.
    if joined.as_os_str().len() > WALKED_PATH_BYTES {
        return Err(Unresolved::Unreachable(std::io::Error::from_raw_os_error(
            libc::ENAMETOOLONG,
        )));
    }
    match std::fs::canonicalize(&joined) {
        Ok(resolved) if workspace.holds(&resolved) => Ok(resolved),
        // Changed since it was walked (a link put on its way): out, where it leads now.
        Ok(_) => Err(Unresolved::Outside),
        Err(e) => Err(Unresolved::Unreachable(e)),
    }
}

/// `path` as the system resolves it, relative to `workspace`'s root if it's relative and
/// there is one, links not followed: what a command reads or makes at a path it's given.
pub(crate) fn joined(workspace: Option<&Workspace>, path: &Path) -> PathBuf {
    match workspace {
        Some(workspace) => workspace.root.join(path),
        None => path.to_owned(),
    }
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

/// Writes `bytes` over `file`, which held `before` when it was read: the file it names
/// (through a symbolic link, which stays), keeping its permissions. Whole, to a new file
/// beside it renamed over it, so a reader never sees a part of it and a write cut short
/// leaves the old file; or, if it has other names (hard links), which a rename would part
/// from it, in place, with the old text kept beside it until the new is on disk, and put
/// back if the write fails (a full disk), so a write cut short never loses the document
/// (killed mid-write, the old text is in the file beside it, `.NAME.fmt-PID`). Refused if
/// this process may not write it, or, as a conflict, if it changed since it was read; and,
/// when it fails, saying whether its own write left the file part written ([`Unsaved`]).
/// What every save of a document goes through: a session's commit, edit, and `format`
/// ([`save_within`], in a workspace).
pub fn save_over(file: &Path, before: &[u8], bytes: &[u8]) -> Result<(), Unsaved> {
    save_within(file, before, bytes, None)
}

/// A session's file in its workspace, for [`save_within`]: the workspace, and the file's
/// path as `open` was given it, which a refusal names.
#[derive(Debug, Clone, Copy)]
pub struct Within<'a> {
    pub workspace: &'a Workspace,
    pub given: &'a Path,
}

/// [`save_over`], and, `within` a workspace, refused if `file` now leads out of it (a link
/// put on its path since it was read: [`Workspace::outside_now`]), checked where the file it
/// writes is found, nothing written.
pub fn save_within(
    file: &Path,
    before: &[u8],
    bytes: &[u8],
    within: Option<Within<'_>>,
) -> Result<(), Unsaved> {
    let _span = tracing::info_span!("save", bytes = bytes.len()).entered();
    let target = match resolve(within.map(|w| w.workspace), file) {
        Ok(target) => target,
        Err(Unresolved::Unreachable(e)) => return Err(Unsaved::from(unwritable(e))),
        Err(Unresolved::Outside) => {
            let given = within.map_or(file, |w| w.given);
            return Err(Unsaved::from(Workspace::outside_now(given)));
        }
    };
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
            .with_suggestion("it was saved by something else meanwhile: read it again, then save"),
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
        .and_then(|()| synced(&f, "the new text beside it", &partial))
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
        })
        .inspect(|()| directory_synced(&target));
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
        .and_then(|()| synced(&copy, "its old text kept beside it", &kept))
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
        synced(file, what, target)
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

/// The directory `target` was renamed in synced, so the rename is on disk: a power cut
/// after the save can't bring back the old file. The file is saved either way, so a
/// directory that can't be opened or synced (a system that doesn't sync directories) is
/// only logged.
fn directory_synced(target: &Path) {
    let Some(directory) = target.parent() else {
        return;
    };
    if let Err(e) = std::fs::File::open(directory)
        .and_then(|opened| synced(&opened, "the directory it was renamed in", directory))
    {
        tracing::warn!(
            error = %e,
            "the file was saved, but its directory couldn't be synced: a power cut soon could bring back the old file"
        );
    }
}

/// `file`, at `path`, on disk, as `sync_all` puts it, said in the log before and after
/// with its path and how long it took (debug): a disk that stalls a save shows as the line
/// it stopped at.
fn synced(file: &std::fs::File, what: &str, path: &Path) -> std::io::Result<()> {
    let path = path.display();
    tracing::debug!(%path, "syncing {what}");
    let start = std::time::Instant::now();
    let synced = file.sync_all();
    let sync_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    match &synced {
        Ok(()) => tracing::debug!(%path, sync_ms, "synced {what}"),
        Err(e) => tracing::debug!(%path, sync_ms, error = %e, "couldn't sync {what}"),
    }
    synced
}

/// How many names [`beside`] tries.
const TRIES: u32 = 64;

/// The longest file name Silico makes, in bytes: Linux's `NAME_MAX`. macOS takes 255
/// UTF-16 units, which 255 bytes never pass.
const NAME_MAX: usize = 255;

/// The `n`th name [`beside`] tries for a file beside one named `name`: `.NAME.fmt-PID`,
/// then `-1`, `-2`, ... after it. A name too long for the last of them is cut short, the
/// same for every try, at a character's end if it's UTF-8 (as APFS requires every name to
/// be), so a document whose own name is as long as a name may be is saved all the same
/// (found by Worker 3), and an error that names the first try names the stem of all. The
/// file beside it is never another's, since each is made new, and a save that leaves it
/// says its name.
fn partial_name(name: &std::ffi::OsStr, n: u32) -> OsString {
    use std::os::unix::ffi::OsStrExt as _;
    let mut suffix = format!(".fmt-{}", std::process::id());
    let longest = suffix.len() + format!("-{}", TRIES - 1).len();
    if n > 0 {
        suffix.push_str(&format!("-{n}"));
    }
    let room = NAME_MAX.saturating_sub(1 + longest);
    let bytes = name.as_bytes();
    let kept = match name.to_str() {
        Some(text) if bytes.len() > room => {
            let mut end = room;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            &bytes[..end]
        }
        _ => &bytes[..bytes.len().min(room)],
    };
    let mut partial = OsString::from(".");
    partial.push(std::ffi::OsStr::from_bytes(kept));
    partial.push(suffix);
    partial
}

/// A new file beside `target` ([`partial_name`]: `.NAME.fmt-PID`, or `-1`, `-2`, ... after
/// it if one is left from a run killed with the same process number), no more open to
/// others than `target` is.
fn beside(target: &Path, metadata: &std::fs::Metadata) -> Result<(PathBuf, std::fs::File), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = directory(target);
    let name = target
        .file_name()
        .ok_or_else(|| unwritable("it has no file name"))?;
    for n in 0..TRIES {
        let partial = dir.join(partial_name(name, n));
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
    let first = dir.join(partial_name(name, 0));
    Err(unwritable(format!(
        "every name for a file beside it is taken ({}, and -1 to -{}): remove those left by runs that were killed",
        shown_path(&first),
        TRIES - 1
    )))
}

/// A path as a message says it: quoted, with control and invisible characters escaped, as an
/// outline quotes a document's text, so a file's name can't act on the terminal it's shown
/// in. Every error and message that names a file says it so. A path longer than any the
/// system takes ([`SHOWN_PATH_CHARS`]), which only a request can give, is cut there, marked
/// `…`, so its refusal isn't as long as it is.
pub(crate) fn shown_path(path: &Path) -> String {
    let shown = path.display().to_string();
    match shown.char_indices().nth(SHOWN_PATH_CHARS) {
        Some((cut, _)) => format!("{}…", crate::command::quoted(&shown[..cut])),
        None => crate::command::quoted(&shown),
    }
}

/// The most of a path a message shows: Linux's `PATH_MAX` (macOS's is 1024), so every path
/// the system can name is shown whole.
const SHOWN_PATH_CHARS: usize = 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_beside_a_long_one_is_cut_to_the_most_a_name_may_be_at_a_characters_end() {
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
        let pid = std::process::id();
        let suffix = format!(".fmt-{pid}");
        // A short name, as it is.
        assert_eq!(
            partial_name(std::ffi::OsStr::new("part.silico"), 0),
            OsString::from(format!(".part.silico{suffix}"))
        );
        assert_eq!(
            partial_name(std::ffi::OsStr::new("part.silico"), 63),
            OsString::from(format!(".part.silico{suffix}-63"))
        );
        // 255 bytes of ASCII: one stem for every try, cut so the last, `-63`, is 255 in all:
        // the dot, the name's first bytes, the suffix (the robustness review: an error names
        // the first try's stem as every try's).
        let long = "a".repeat(255);
        let kept = 255 - 1 - suffix.len() - "-63".len();
        for (n, tail) in [
            (0, suffix.clone()),
            (1, format!("{suffix}-1")),
            (63, format!("{suffix}-63")),
        ] {
            let cut = partial_name(std::ffi::OsStr::new(&long), n).into_vec();
            assert_eq!(cut, format!(".{}{tail}", &long[..kept]).into_bytes(), "{n}");
        }
        assert_eq!(partial_name(std::ffi::OsStr::new(&long), 63).len(), 255);
        // Two-byte characters, cut where a character ends: valid UTF-8, one byte short when
        // the room ends inside one. Two names, one a byte out of step with the other, so one
        // of them needs that, whatever the process number's length makes the room (the
        // mutation run: with an even room, `"é"` alone never did).
        let room = 255 - 1 - suffix.len() - "-63".len();
        for (name, inside) in [
            ("é".repeat(127), room % 2),
            (format!("a{}", "é".repeat(127)), (room - 1) % 2),
        ] {
            let cut =
                String::from_utf8(partial_name(std::ffi::OsStr::new(&name), 0).into_vec()).unwrap();
            assert_eq!(
                cut,
                format!(".{}{suffix}", &name[..room - inside]),
                "{name}"
            );
        }
        // Not UTF-8 (Linux allows it): cut by bytes, to 255.
        let raw = std::ffi::OsStr::from_bytes(&[0xff; 255]);
        let cut = partial_name(raw, 63).into_vec();
        assert_eq!(cut.len(), 255);
        assert!(cut.ends_with(format!("{suffix}-63").as_bytes()));
        assert!(cut[1..room + 1].iter().all(|b| *b == 0xff));
    }

    #[test]
    fn a_file_written_beside_one_it_replaces_is_made_no_more_open_than_it() {
        // Its permission bits alone, as they are: a mode's type bits (a regular file's
        // 0o100000) aren't permissions, and none is added. With none to replace, rw-r--r--.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("silico-partial-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("view.png");
        std::fs::write(&file, b"old").unwrap();
        for mode in [0o600, 0o640, 0o444, 0o751] {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let meta = std::fs::metadata(&file).unwrap();
            assert_eq!(partial_mode(Some(&meta)), mode, "{mode:o}");
        }
        assert_eq!(partial_mode(None), 0o644);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A scratch directory (as the system spells it, through its links: on macOS,
    /// `/private/var/...`, given here as `/var/...` too) holding `work`, the workspace, with
    /// `part.silico` and a directory `sub` in it, and `outside`, a directory beside it with a
    /// directory `a` in it.
    fn scratch() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("work/sub")).unwrap();
        std::fs::write(dir.path().join("work/part.silico"), b"{}").unwrap();
        std::fs::create_dir_all(dir.path().join("outside/a")).unwrap();
        let workspace = Workspace::new(&dir.path().join("work")).unwrap();
        (dir, workspace)
    }

    #[test]
    fn a_path_that_doesnt_resolve_leads_where_its_links_and_dots_take_it_however_deep() {
        use std::os::unix::fs::symlink;
        let (dir, w) = scratch();
        let work = dir.path().join("work");
        let deep = "m/".repeat(100);
        // Below an outside directory that's there, or one that isn't: out either way, at
        // any depth (each missing directory looked at once, none of them a link).
        for top in ["outside", "nowhere", "outside/a"] {
            assert!(
                w.leads_out(&work.join(format!("../{top}/{deep}x"))),
                "{top}"
            );
            assert!(w.leads_out(&work.join(format!("../{top}/x"))), "{top}");
        }
        // Below the workspace: in it, at any depth.
        assert!(!w.leads_out(&work.join(format!("{deep}x"))));
        assert!(!w.leads_out(&work.join(format!("sub/{deep}x"))));
        assert!(!w.leads_out(&work.join(format!("sub/../{deep}x"))));
        // Out and back in: in.
        assert!(!w.leads_out(&work.join(format!("../work/{deep}x"))));
        // Below a file, or through one and up: stopped at the file, never walked on (macOS's
        // `realpath` would go on, outside, by the `..`'s text).
        assert_eq!(w.walk(&work.join("part.silico/x")), Walked::ThroughAFile);
        assert_eq!(
            w.walk(&work.join("part.silico/../../outside/x")),
            Walked::ThroughAFile
        );
        assert_eq!(w.walk(&work.join("part.silico/..")), Walked::ThroughAFile);
        assert_eq!(w.walk(&work.join("part.silico")), Walked::In);
        // Through links in the workspace: one to the outside directory, one to where nothing
        // is outside, one to a directory in the workspace.
        symlink(dir.path().join("outside"), work.join("out")).unwrap();
        symlink("../nowhere", work.join("gone")).unwrap();
        symlink("sub", work.join("in")).unwrap();
        for link in ["out", "gone"] {
            assert!(w.leads_out(&work.join(format!("{link}/{deep}x"))), "{link}");
            assert!(w.leads_out(&work.join(link)), "{link}");
        }
        assert!(!w.leads_out(&work.join(format!("in/{deep}x"))));
        // A `..` after a link is the directory above where the link leads, as the system
        // has it: `out/a/..` is `outside`, so `out/a/../x` is out, though `work/x` isn't.
        assert!(w.leads_out(&work.join("out/a/../x")));
        assert!(!w.leads_out(&work.join("in/../x")));
        assert!(w.leads_out(&work.join("in/../../nowhere/x")));
    }

    #[test]
    fn a_file_put_where_the_root_or_an_ancestor_was_stops_the_walk_there() {
        // A workspace at `top/mid/work`, then `mid` (with the root in it) or the root alone
        // removed and a file put in its place: a path through it with more to go stops at
        // the file, as the system stops (ENOTDIR), never walked on or left to
        // `canonicalize`; one that ends at it is in, the system's to say.
        for gone in ["top/mid", "top/mid/work"] {
            let dir = tempfile::tempdir().unwrap();
            let top = std::fs::canonicalize(dir.path()).unwrap();
            std::fs::create_dir_all(top.join("top/mid/work")).unwrap();
            std::fs::create_dir_all(top.join("top/outside")).unwrap();
            let w = Workspace::new(&top.join("top/mid/work")).unwrap();
            std::fs::remove_dir_all(top.join(gone)).unwrap();
            std::fs::write(top.join(gone), b"a file").unwrap();
            let at = top.join(gone);
            assert_eq!(w.walk(&at.join("x")), Walked::ThroughAFile, "{gone}");
            assert_eq!(
                w.walk(&at.join("x/../../outside/y")),
                Walked::ThroughAFile,
                "{gone}"
            );
            assert_eq!(w.walk(&at.join("..")), Walked::ThroughAFile, "{gone}");
            assert_eq!(w.walk(&at), Walked::In, "{gone}");
            assert!(matches!(
                resolve(Some(&w), &w.root().join("x/..")),
                Err(Unresolved::Unreachable(e)) if e.raw_os_error() == Some(libc::ENOTDIR)
            ));
            // Another name of an ancestor that's a file is out, as one where nothing is:
            // nothing outside is told apart by what's there.
            std::fs::write(top.join("top/secret"), b"a file").unwrap();
            assert_eq!(w.walk(&top.join("top/secret/x")), Walked::Out, "{gone}");
            assert_eq!(w.walk(&top.join("top/nowhere/x")), Walked::Out, "{gone}");
        }
    }

    #[test]
    fn links_alone_spend_the_budget_of_forty() {
        use std::os::unix::fs::symlink;
        let (_dir, w) = scratch();
        // As the system spells it, so its form is the root's.
        let work = w.root().to_owned();
        let target = work.join("../outside/made");
        // A chain of n links in the workspace, the last to `outside/made`, which isn't there:
        // 40 followed, the system's own limit, and it's out; 41, and it's decided by the
        // path's form, `work/c41-0`, which is in.
        for n in [1, 39, 40, 41, 60] {
            for i in 0..n {
                let to = if i + 1 == n {
                    target.clone()
                } else {
                    PathBuf::from(format!("c{n}-{}", i + 1))
                };
                symlink(to, work.join(format!("c{n}-{i}"))).unwrap();
            }
            let first = work.join(format!("c{n}-0"));
            assert_eq!(w.leads_out(&first), n <= 40, "{n}");
            // And as many missing directories below as there are: no more spent.
            assert_eq!(
                w.leads_out(&first.join("m/".repeat(100)).join("x")),
                n <= 40,
                "{n}"
            );
        }
        // A loop: decided by its form, in.
        symlink("loop-b", work.join("loop-a")).unwrap();
        symlink("loop-a", work.join("loop-b")).unwrap();
        assert!(!w.leads_out(&work.join("loop-a")));
        assert!(!w.leads_out(&work.join("loop-a/x")));
        assert!(!target.exists());
    }

    #[test]
    fn a_path_longer_than_the_system_resolves_is_decided_by_its_form_at_once() {
        use std::os::unix::fs::symlink;
        let (dir, w) = scratch();
        // As the system spells it, so its form is the root's.
        let work = w.root().to_owned();
        // Twenty million bytes, four million names: the `..`s taken as written, and nothing
        // looked at (`out` is a link out of the workspace, but not followed).
        symlink(dir.path().join("outside"), work.join("out")).unwrap();
        let churn = "x/../".repeat(4_000_000);
        assert!(w.leads_out(&work.join(format!("{churn}../outside/x"))));
        assert!(w.leads_out(&work.join(format!("{churn}../nowhere/x"))));
        assert!(!w.leads_out(&work.join(format!("{churn}sub/x"))));
        assert!(!w.leads_out(&work.join(format!("{churn}out/x"))));
        assert!(w.leads_out(&work.join(format!("{}x", "../".repeat(2000)))));
        // Just within the length walked, the link is followed.
        let near = work.join("out").join("m/".repeat(1000)).join("x");
        assert!(near.as_os_str().len() <= WALKED_PATH_BYTES);
        assert!(w.leads_out(&near));
    }

    #[test]
    fn the_walk_stops_at_exactly_the_longest_path_the_system_resolves() {
        use std::os::unix::fs::symlink;
        let (dir, w) = scratch();
        let work = w.root().to_owned();
        // Through `out`, a link out of the workspace: out if walked, in by its form.
        symlink(dir.path().join("outside"), work.join("out")).unwrap();
        let mut below = work.join("out");
        while below.as_os_str().len() < WALKED_PATH_BYTES - 200 {
            below.push("m");
        }
        // `below/` and a name making it `bytes` long.
        let of = |bytes: usize| {
            let name = "n".repeat(bytes - below.as_os_str().len() - 1);
            let path = below.join(name);
            assert_eq!(path.as_os_str().len(), bytes);
            path
        };
        assert!(w.leads_out(&of(WALKED_PATH_BYTES)));
        assert!(!w.leads_out(&of(WALKED_PATH_BYTES + 1)));
        assert!(matches!(
            resolve(Some(&w), &of(WALKED_PATH_BYTES)),
            Err(Unresolved::Outside)
        ));
        // One past, in by its form: refused as too long, as the system refuses it.
        match resolve(Some(&w), &of(WALKED_PATH_BYTES + 1)) {
            Err(Unresolved::Unreachable(e)) => {
                assert_eq!(e.raw_os_error(), Some(libc::ENAMETOOLONG));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_path_of_exactly_the_longest_walked_is_left_to_the_system_and_one_byte_more_isnt() {
        // Found by the M4 milestone's mutation run: refusing a path of exactly
        // WALKED_PATH_BYTES as too long, unresolved, passed every test (the one above takes
        // that length out through a link first). A path in the workspace, `sub/../` again
        // and again and then a file's name, its `..`s taking it back to the root, so each
        // name the system looks up is short: of 4096 bytes, walked, in, and then resolved
        // by the system. glibc's `realpath` resolves it, a name at a time, to the file
        // (checked on Debian's glibc 2.41); macOS's refuses any path past its own PATH_MAX,
        // 1024 bytes, before looking at it. Of 4097, decided by its form, in, and refused
        // as too long without asking the system, on either.
        let (_dir, w) = scratch();
        let work = w.root().to_owned();
        let room = work.as_os_str().len() + 1;
        // The relative path, of `bytes` joined to the root, and the file it leads to.
        let of = |bytes: usize| {
            let churn = (bytes - room - 100) / 7;
            let name = "p".repeat(bytes - room - 7 * churn);
            std::fs::write(work.join(&name), b"{}").unwrap();
            let path = PathBuf::from(format!("{}{name}", "sub/../".repeat(churn)));
            assert_eq!(work.join(&path).as_os_str().len(), bytes);
            (path, work.join(name))
        };
        let (path, file) = of(WALKED_PATH_BYTES);
        assert!(!w.leads_out(&work.join(&path)));
        match resolve(Some(&w), &path) {
            Ok(resolved) if cfg!(target_os = "linux") => assert_eq!(resolved, file),
            Err(Unresolved::Unreachable(e)) if cfg!(target_os = "macos") => {
                assert_eq!(e.raw_os_error(), Some(libc::ENAMETOOLONG));
            }
            other => panic!("{other:?}"),
        }
        let (longer, _) = of(WALKED_PATH_BYTES + 1);
        assert!(!w.leads_out(&work.join(&longer)));
        match resolve(Some(&w), &longer) {
            Err(Unresolved::Unreachable(e)) => {
                assert_eq!(e.raw_os_error(), Some(libc::ENAMETOOLONG));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_roots_held_spelling_is_the_directorys_own_and_a_files_is_none() {
        // Found by the M4 milestone's mutation run: none for the directory itself (the
        // device and inode check turned), or a spelling for a file (opened without
        // O_DIRECTORY), passed every test; on Linux the handle's spelling is the canonical
        // root's, so nothing that compares paths with both tells. A plain directory, reached
        // through no link: the system names its handle as `canonicalize` names it. A file at
        // the root's path (the root replaced since it was checked to be a directory): no
        // handle on a directory, so no spelling.
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        let work = real.join("work");
        std::fs::create_dir(&work).unwrap();
        assert_eq!(held_spelling(&work), Some(work.clone()));
        let file = real.join("file");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(held_spelling(&file), None);
    }

    #[test]
    fn a_path_is_in_the_workspace_below_either_spelling_of_its_root() {
        // Found by the M4 milestone's mutation run: holding only a path below both
        // spellings passed every test, as on Linux the two are one. Below the canonical
        // root, or the root as the system names its handle (macOS's firmlinks), it's in;
        // below neither, it isn't. With no held spelling (a system that can't say where a
        // handle is), the canonical root stands alone.
        let both = Workspace {
            root: PathBuf::from("/top/canonical"),
            as_held: Some(PathBuf::from("/top/held")),
        };
        assert!(both.holds(Path::new("/top/canonical/x")));
        assert!(both.holds(Path::new("/top/held/x")));
        assert!(!both.holds(Path::new("/top/other/x")));
        let alone = Workspace {
            root: PathBuf::from("/top/canonical"),
            as_held: None,
        };
        assert!(alone.holds(Path::new("/top/canonical/x")));
        assert!(!alone.holds(Path::new("/top/held/x")));
    }

    #[test]
    fn a_resolve_raced_by_a_directory_flipping_with_a_link_out_never_resolves_outside() {
        // Found by the M4 milestone's mutation run: taking the system's resolution without
        // checking it again passed every test. `sub/part.silico` is walked a name at a time,
        // then resolved by the system; with `work/sub` exchanged, again and again, with
        // `work/link` (a link to `outside/a`), the walk can find the directory and the
        // system the link, and resolve to `outside/a/part.silico`, which must be refused,
        // out, as it leads now. A race, so a loop: resolves, as another thread flips the two
        // at uneven intervals, until one resolves outside, 300,000 resolves, or 20 seconds,
        // whichever is first. A pass is no proof, but a failure is one.
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (dir, w) = scratch();
        let work = w.root().to_owned();
        let outside = std::fs::canonicalize(dir.path()).unwrap().join("outside");
        std::fs::write(work.join("sub/part.silico"), b"{}").unwrap();
        std::fs::write(outside.join("a/part.silico"), b"{}").unwrap();
        std::os::unix::fs::symlink(outside.join("a"), work.join("link")).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flipper = {
            let (stop, work) = (Arc::clone(&stop), work.clone());
            std::thread::spawn(move || {
                let mut k: u64 = 0;
                while !stop.load(Ordering::Relaxed) {
                    renameat_with(
                        CWD,
                        work.join("sub"),
                        CWD,
                        work.join("link"),
                        RenameFlags::EXCHANGE,
                    )
                    .unwrap();
                    k = (k + 7919) % 4001;
                    for _ in 0..k {
                        std::hint::spin_loop();
                    }
                }
            })
        };
        let began = std::time::Instant::now();
        let mut escaped = None;
        for i in 0..300_000 {
            if let Ok(resolved) = resolve(Some(&w), Path::new("sub/part.silico"))
                && !resolved.starts_with(&work)
            {
                escaped = Some((i, resolved));
                break;
            }
            if began.elapsed() > std::time::Duration::from_secs(20) {
                break;
            }
        }
        stop.store(true, Ordering::Relaxed);
        flipper.join().unwrap();
        assert_eq!(escaped, None, "a path resolved outside the workspace");
    }

    #[test]
    fn a_name_outside_is_never_looked_up_whether_or_not_its_there() {
        use std::os::unix::fs::PermissionsExt as _;
        let (dir, w) = scratch();
        let work = w.root().to_owned();
        let top = work.parent().unwrap().to_owned();
        // `outside` is there, `nowhere` isn't, and `locked` can't be looked in: each the
        // same, out, through it and back or not.
        let locked = top.join("locked");
        std::fs::create_dir_all(locked.join("in")).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut decided = Vec::new();
        for name in ["outside", "nowhere", "locked"] {
            for rest in ["../work/part.silico", "in/part.silico", "", "x/../../work"] {
                let path = work.join(format!("../{name}/{rest}"));
                decided.push((path.clone(), w.leads_out(&path), resolve(Some(&w), &path)));
                let absolute = top.join(name).join(rest);
                decided.push((
                    absolute.clone(),
                    w.leads_out(&absolute),
                    resolve(Some(&w), &absolute),
                ));
            }
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        for (path, out, resolved) in decided {
            assert!(out, "{}", path.display());
            assert!(
                matches!(resolved, Err(Unresolved::Outside)),
                "{}: {resolved:?}",
                path.display()
            );
        }
        // Down through the root's ancestors, by its own spelling or the system's other
        // (macOS's `/var` for `/private/var`): in.
        let part = work.join("part.silico");
        assert_eq!(resolve(Some(&w), &part).unwrap(), part);
        let given = dir.path().join("work/../work/part.silico");
        assert!(!w.leads_out(&given));
        assert_eq!(resolve(Some(&w), &given).unwrap(), part);
        // The root's parent, and the top: out, as ancestors are, nothing below looked at.
        assert!(w.leads_out(&top));
        assert!(w.leads_out(Path::new("/")));
    }

    #[test]
    fn a_path_in_a_workspace_gone_is_never_out_of_it() {
        let (dir, w) = scratch();
        let work = dir.path().join("work");
        std::fs::remove_dir_all(&work).unwrap();
        for path in ["x", "sub/x", "m/m/m/x", ""] {
            assert!(!w.leads_out(&work.join(path)), "{path}");
        }
        // A path through the workspace gone goes no further, as the system's doesn't; one
        // beside it is out.
        assert!(!w.leads_out(&work.join("../outside/x")));
        assert!(w.leads_out(&dir.path().join("outside/x")));
        assert!(w.leads_out(&dir.path().join("nowhere/x")));
    }

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

    #[cfg(target_os = "macos")]
    #[test]
    fn a_link_that_cant_be_read_stops_the_walk_where_it_is() {
        // Found by the M4 milestone's mutation run: deciding a link the system shows but
        // won't read the wrong way round (out in the workspace, in in an ancestor) passed
        // every test. macOS checks a link's own read permission: one of mode 000 is listed but
        // can't be read (EACCES). The walk stops at it: in the workspace, in (the system's to
        // say what's wrong); in an ancestor, out. Each link leads the other way, should it be
        // followed: `work/locked` out, `locked` beside the root back in to it.
        use rustix::fs::{AtFlags, CWD, Mode, chmodat};
        let (_dir, w) = scratch();
        let work = w.root().to_owned();
        let top = work.parent().unwrap().to_owned();
        std::os::unix::fs::symlink("../outside", work.join("locked")).unwrap();
        std::os::unix::fs::symlink("work", top.join("locked")).unwrap();
        for link in [work.join("locked"), top.join("locked")] {
            chmodat(CWD, &link, Mode::empty(), AtFlags::SYMLINK_NOFOLLOW).unwrap();
            assert!(
                std::fs::read_link(&link).is_err(),
                "a link of mode 000 is still readable (run as root?): this case can't be tested"
            );
        }
        assert_eq!(w.walk(&work.join("locked")), Walked::In);
        assert_eq!(w.walk(&work.join("locked/x")), Walked::In);
        assert_eq!(w.walk(&top.join("locked")), Walked::Out);
        assert_eq!(w.walk(&top.join("locked/part.silico")), Walked::Out);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_link_that_cant_be_read_stops_the_walk_where_it_is() {
        // Found by the M4 milestone's mutation run: as macOS's test. Linux ignores a link's
        // permissions, but /proc/1/cwd, init's directory, is a link it lists and won't read
        // to another user (EACCES: no ptrace access to root's process). In the workspace
        // /proc/1: in (followed, it would lead out, to /). In the workspace /proc/1/task,
        // where /proc/1 is an ancestor: out. Run as root, or where process 1 is this user's
        // (a process namespace of its own), the link can be read, and the test says so.
        let init = Path::new("/proc/1");
        assert!(
            std::fs::symlink_metadata(init.join("cwd"))
                .unwrap_or_else(|e| panic!(
                    "/proc/1/cwd can't be seen ({e}: /proc mounted with hidepid?): this case \
                     can't be tested"
                ))
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_link(init.join("cwd")).is_err(),
            "/proc/1/cwd is readable (run as root, or in a process namespace of this user's?): \
             this case can't be tested"
        );
        let w = Workspace::new(init).unwrap();
        assert_eq!(w.walk(&init.join("cwd")), Walked::In);
        assert_eq!(w.walk(&init.join("cwd/x")), Walked::In);
        let w = Workspace::new(&init.join("task")).unwrap();
        assert_eq!(w.walk(&init.join("cwd")), Walked::Out);
    }

    #[cfg(target_os = "linux")]
    const BIND_ENV: &str = "SILICO_FILE_TEST_BIND";

    /// Not a test of its own: run by [`the_way_to_the_root_by_a_bind_mount_of_it_goes_on_to_it`]
    /// in a user and mount namespace of its own, with [`BIND_ENV`] set. Run normally, it does
    /// nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn bind_entry() {
        use std::os::unix::fs::MetadataExt as _;
        let Some(top) = std::env::var_os(BIND_ENV) else {
            return;
        };
        let top = std::fs::canonicalize(top).unwrap();
        let (root, alias) = (top.join("work"), top.join("alias"));
        // `alias` is `work` mounted there: one directory, its device and inode, by two names.
        let (r, a) = (
            std::fs::symlink_metadata(&root).unwrap(),
            std::fs::symlink_metadata(&alias).unwrap(),
        );
        assert_eq!(
            (a.dev(), a.ino()),
            (r.dev(), r.ino()),
            "the bind mount isn't the workspace's directory"
        );
        let w = Workspace::new(&root).unwrap();
        assert_eq!(w.walk(&alias.join("part.silico")), Walked::In);
        assert_eq!(w.walk(&alias), Walked::In);
        // Said only past every check, so the run outside can tell it from the early return.
        let _ = std::io::Write::write_all(&mut std::io::stderr(), b"bind checked\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_way_to_the_root_by_a_bind_mount_of_it_goes_on_to_it() {
        // Found by the M4 milestone's mutation run: going on only by the root's own name (the
        // device and inode check dropped, or required beside the name) passed every test.
        // The workspace's directory bound at a second name, in a user and mount namespace of
        // the test's own (`unshare -Urm`, as an unprivileged user may, as
        // `a_file_on_another_device_with_a_held_documents_inode_is_another_file` does): the
        // walk goes on through that name, the same directory, as through the root's own. Run
        // in [`bind_entry`], this binary run again inside the namespace after the shell there
        // makes the mount. A namespace that can't be made fails the test, saying why.
        let dir = tempfile::tempdir().unwrap();
        for name in ["work", "alias"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        let me = std::env::current_exe().unwrap();
        let run = std::process::Command::new("unshare")
            .args(["-Urm", "sh", "-c"])
            .arg(
                "mount --bind \"$1\" \"$2\" && \
                 exec \"$0\" --exact file::tests::bind_entry --nocapture --test-threads=1",
            )
            .arg(&me)
            .arg(dir.path().join("work"))
            .arg(dir.path().join("alias"))
            .env(BIND_ENV, dir.path())
            .output()
            .unwrap_or_else(|e| panic!("unshare couldn't be run: {e}"));
        let (out, err) = (
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr),
        );
        assert!(
            run.status.success(),
            "{}\nstdout:\n{out}\nstderr:\n{err}",
            run.status
        );
        assert!(
            out.contains("test file::tests::bind_entry ... ok") && err.contains("bind checked"),
            "stdout:\n{out}\nstderr:\n{err}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_way_to_the_root_by_another_spelling_of_the_same_directory_goes_on_to_it() {
        // Found by the M4 milestone's mutation run: going on only by the root's own
        // spelling (the device and inode check dropped, or required beside the spelling)
        // passed every test, as on Linux no other name of an ancestor is the same directory.
        // The workspace `café`, its é one character (NFC), reached by `café` with an e and a
        // combining accent (NFD): on macOS (APFS, insensitive to Unicode normalization,
        // whatever its case-sensitivity) the same directory, so the way to the root: in, and
        // resolved to the root's own spelling.
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(dir.path()).unwrap();
        let root = top.join("caf\u{e9}");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("part.silico"), b"{}").unwrap();
        let w = Workspace::new(&root).unwrap();
        let other = top.join("cafe\u{301}");
        assert_ne!(other, root);
        let (found, root_meta) = (
            std::fs::symlink_metadata(&other).unwrap(),
            std::fs::symlink_metadata(&root).unwrap(),
        );
        assert_eq!(
            (found.dev(), found.ino()),
            (root_meta.dev(), root_meta.ino()),
            "{} isn't {} here: not APFS?",
            other.display(),
            root.display()
        );
        let part = other.join("part.silico");
        assert_eq!(w.walk(&part), Walked::In);
        assert_eq!(w.walk(&other), Walked::In);
        assert_eq!(resolve(Some(&w), &part).unwrap(), root.join("part.silico"));
    }
}

/// The rules a file a command writes is held to (decision 35: `render`'s `to`,
/// `export`'s `path`; docs/design/m4.1-command-api.md, *Files a command writes*), and a
/// document a commit saves elsewhere (Save As, M5.4: `commit`'s `path`, [`document`]
/// true).
///
/// [`document`]: Self::document
#[derive(Debug, Clone, Copy)]
pub struct Output<'a> {
    /// The request's field the path came in (`to`, `path`), which every refusal names.
    pub field: &'a str,
    /// The format's extensions, without their dots (`png`; `step`, `stp`): a path must end
    /// in one, any case.
    pub extensions: &'a [&'a str],
    /// What writes it, as a refusal says it (`an export`, `a render`).
    pub writer: &'a str,
    /// Whether a file already at the path may be replaced.
    pub replace: bool,
    /// The workspace the path must be inside, never through a link out of it, if the
    /// sessions have one (decision 26): a relative path is relative to its root.
    pub workspace: Option<&'a Workspace>,
    /// The documents the sessions hold, which it may never be.
    pub held: &'a [PathBuf],
    /// Whether what's written is a document (a Save As), whose extension is `.silico`:
    /// otherwise a path that is a document, or links to one, is refused.
    pub document: bool,
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
    let joined = joined(rules.workspace, given);
    let path = joined.as_path();
    let refuse = |why: String, suggestion: &str| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("{} {}: {why}", rules.field, shown_path(given)),
        )
        .with_parameter(rules.field)
        .with_suggestion(suggestion)
    };
    let has = |p: &Path, exts: &[&str]| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.iter().any(|ext| e.eq_ignore_ascii_case(ext)))
    };
    // ".png"; ".step or .stp".
    let ends = rules
        .extensions
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(" or ");
    // A document, however it's reached: never written over, each said alike.
    let a_document = |what: &str, suggestion: String| {
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} {what}: {} never writes over one",
                shown_path(given),
                rules.writer
            ),
        )
        .with_parameter(rules.field)
        .with_suggestion(suggestion)
    };
    if !rules.document && has(path, &["silico"]) {
        return Err(a_document(
            "is a document",
            format!("give a path ending in {ends}"),
        ));
    }
    if !has(path, rules.extensions) {
        return Err(refuse(
            format!("doesn't end in {ends}"),
            &format!("give a path ending in {ends}"),
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
    // Out of the workspace whether or not its directory is there, so nothing outside it is
    // told apart by whether it exists.
    let dir = resolve(rules.workspace, parent).map_err(|unresolved| match unresolved {
        Unresolved::Outside => Workspace::outside(given, rules.field),
        Unresolved::Unreachable(e) => refuse(
            format!("its directory can't be reached: {e}"),
            "give a path in a directory that exists",
        ),
    })?;
    // Where it would be written: through a link already at the path, to what it names.
    let existing = std::fs::symlink_metadata(dir.join(name)).ok();
    let target = match &existing {
        // Through a link at the path as every path is resolved: in a workspace, never
        // looking outside it.
        Some(meta) => match resolve(rules.workspace, &dir.join(name)) {
            Ok(target) => target,
            Err(Unresolved::Outside) => return Err(Workspace::outside(given, rules.field)),
            Err(Unresolved::Unreachable(e)) if meta.file_type().is_symlink() => {
                return Err(refuse(
                    format!("is a link to nothing that can be reached: {e}"),
                    "give a path that isn't a broken link",
                ));
            }
            // Gone since it was looked at: as if it hadn't been there (the write checks
            // what's there again).
            Err(Unresolved::Unreachable(_)) => dir.join(name),
        },
        None => dir.join(name),
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
    if !rules.document && has(&target, &["silico"]) {
        return Err(a_document(
            "is a link to a document",
            "write it to a path that isn't a document's".to_owned(),
        ));
    }
    // A link to a file of another format (a clone's `view.png` linked to its README) would
    // be replaced by this one's bytes.
    if !has(&target, rules.extensions) {
        return Err(refuse(
            format!("is a link to a file that isn't a {ends}"),
            "write it to a path that isn't a link to another kind of file",
        ));
    }
    // A held document is the file, not a spelling of its path: whatever name reaches it (a
    // hard link's, or another case of its own on a case-insensitive file system, which
    // Linux's `realpath` keeps as given), it's the device and inode of one held. Both, since
    // inode numbers repeat across file systems: a file on another volume may have a held
    // document's inode. The device's half is tested on Linux only, where a test makes two
    // file systems of its own (tests/file.rs, two tmpfs mounts in a namespace of its own).
    let there = std::fs::metadata(&target);
    if let Ok(meta) = &there
        && rules.held.iter().any(|held| {
            std::fs::metadata(held).is_ok_and(|h| h.dev() == meta.dev() && h.ino() == meta.ino())
        })
    {
        return Err(a_document(
            "is a document a session holds",
            "write it to a path that isn't a document's".to_owned(),
        ));
    }
    let replaced = match there {
        Ok(meta) if !meta.is_file() => {
            return Err(refuse(
                "is there, and isn't a file".to_owned(),
                "give the path of a file to write",
            ));
        }
        Ok(_) if !rules.replace => return Err(exists(given, rules.field)),
        Ok(meta) => Some(meta),
        Err(_) => None,
    };
    Ok((target, replaced))
}

/// The refusal of a path a command writes, given in `field`, where a file is already there
/// and the request didn't say to replace it.
pub(crate) fn exists(given: &Path, field: &str) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("{} exists", shown_path(given)),
    )
    .with_parameter(field)
    .with_suggestion(REPLACE_ADVICE)
}

/// What a file already at a path a command writes, without `replace`, suggests: a
/// client with its own way of saying `replace` (the CLI's `--replace`) says it its way.
pub const REPLACE_ADVICE: &str = "give replace: true to replace it, or another path";

/// The permissions [`write_beside`] makes its new file with: those of the file it replaces,
/// if one, without its type's bits (so never more open than it while it's written), or
/// `rw-r--r--`.
fn partial_mode(replaced: Option<&std::fs::Metadata>) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    replaced.map_or(0o644, |m| m.permissions().mode() & 0o777)
}

/// `bytes` written to `target` through a new file beside it renamed into place, with the
/// permissions of the file it replaces, if one (or the new file's own); the directory it's
/// renamed in synced after, as a save's is, so a power cut can't bring back the old file.
fn write_beside(
    target: &Path,
    replaced: Option<&std::fs::Metadata>,
    bytes: &[u8],
) -> Result<(), Error> {
    let mode = partial_mode(replaced);
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
        .and_then(|()| synced(&f, "the new file", &partial))
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
        })
        .inspect(|()| directory_synced(target));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written
}
