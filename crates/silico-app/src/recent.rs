//! The documents opened lately: the start page's Recent (docs/design/m5-ui/DESIGN.md,
//! *Start page or blank canvas?*), kept in a file of the app's own in the platform's
//! app-data folder ([`Recents::default_file`]).
//!
//! The file is read, and each document in it looked for (whether it's there, and when it was
//! last modified), on a thread of its own, which reads it again and writes it after each
//! change, so the UI thread never waits on it (invariant 4), and what another instance of
//! the app wrote meanwhile is kept: the window asks the thread ([`Recents::opened`],
//! [`Recents::check`], [`Recents::remove`], [`Recents::look`]) and draws the list it last
//! told ([`Recents::list`]). The times are read from the files themselves, never kept in
//! the list's file. Its form is a header line and an absolute path a line, newest first
//! (`text`); a file that isn't one (`parse`), or can't be read, is an empty list, never a
//! failure, and at most [`MAX_FILE_BYTES`] of it is read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use jiff::Timestamp;
use jiff::tz::TimeZone;

pub use crate::app_data::Read;
use crate::app_data::{read_capped, write};
use crate::wake::Wake;

/// How many documents the list keeps.
pub const KEPT: usize = 10;

/// The list's file's name in the app-data folder.
pub const FILE_NAME: &str = "recent-documents";

/// The file's first line: its form, and the form's version.
pub(crate) const HEADER: &str = "silico recent-documents 1";

/// The longest path the list keeps, in bytes: Linux's `PATH_MAX` (macOS's is shorter). A
/// longer line spoils the file, as any line that isn't a path does.
pub const MAX_PATH_BYTES: usize = 4096;

/// The most of the file read: [`KEPT`] paths of [`MAX_PATH_BYTES`] each fit with room to
/// spare. Whatever comes after it, and the line it cuts, is left unread.
pub const MAX_FILE_BYTES: usize = 64 * 1024;

/// A document in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    /// Its path, absolute.
    pub path: String,
    /// Whether nothing was at its path when the list was read.
    pub missing: bool,
    /// When its file was last modified, as it was last looked at: none until it has been, if
    /// it's missing, or if the file system gives no time. Said in [`Recents::zone`].
    pub modified: Option<Timestamp>,
}

impl Recent {
    /// Its file's name, as its tab names it.
    pub fn name(&self) -> &str {
        name_of(&self.path)
    }

    /// The folder it's in.
    pub fn folder(&self) -> &str {
        Path::new(&self.path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("")
    }
}

/// The name of the file at `path`, as a tab names it: `path` itself if it names none.
pub fn name_of(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
}

/// What the window asks the list's thread.
enum Ask {
    /// A document opened (or saved as), at this path as it was given: first in the list now.
    Opened(String),
    /// A document whose open was refused: dropped from the list if nothing is at its path.
    Check(String),
    /// A document dropped from the list, whatever is at its path (Remove from list).
    Remove(String),
    /// Each document in the list looked for again: whether it's there, and when it was
    /// modified.
    Look,
}

/// What the list's thread tells the window.
enum Told {
    /// The list, as it is now, and the platform's time zone its times are said in.
    List(Vec<Recent>, TimeZone),
    /// A document dropped from the list because nothing was at its path.
    Gone(Recent),
    /// A document dropped from the list as asked ([`Ask::Remove`]), its file written.
    Removed(String),
    /// A document asked to be dropped ([`Ask::Remove`]) whose list's file couldn't be
    /// written: still listed, as the file has it.
    NotRemoved(String),
}

/// What the list's thread told since the last poll, besides the list.
#[derive(Debug, Default)]
pub(crate) struct Polled {
    /// The documents dropped because nothing was at their paths.
    pub gone: Vec<Recent>,
    /// The paths removed from the list as asked ([`Recents::remove`]).
    pub removed: Vec<String>,
    /// The paths asked to be removed that stay listed: the list's file couldn't be written.
    pub not_removed: Vec<String>,
}

/// The list of recent documents, as the window last heard it from the thread that keeps it.
pub struct Recents {
    list: Vec<Recent>,
    /// The platform's time zone, as the thread last read it with the list.
    zone: TimeZone,
    read: bool,
    asks: Option<Sender<Ask>>,
    told: Option<Receiver<Told>>,
    wake: Wake,
}

impl Recents {
    /// The list kept in `file`, read on a thread of its own (`app_data::read_capped`: a
    /// regular file, never a pipe that could hang it, and at most [`MAX_FILE_BYTES`] of it).
    pub fn at(file: PathBuf) -> Self {
        Self::reading(file, Box::new(|file| read_capped(file, MAX_FILE_BYTES)))
    }

    /// The list kept in `file`, read first by `read` on a thread of its own (each later
    /// read, before a write, `app_data::read_capped`'s).
    pub fn reading(file: PathBuf, read: Read) -> Self {
        let (asks, asked) = mpsc::channel();
        let (tell, told) = mpsc::channel();
        let wake = Wake::default();
        let waking = wake.clone();
        let started = std::thread::Builder::new()
            .name("silico-recent".to_owned())
            .spawn(move || keep(&file, read, &asked, &tell, &waking));
        match started {
            Ok(_) => Recents {
                list: Vec::new(),
                zone: TimeZone::UTC,
                read: false,
                asks: Some(asks),
                told: Some(told),
                wake,
            },
            Err(e) => {
                tracing::warn!("the list of recent documents can't be kept: {e}");
                Self::unkept()
            }
        }
    }

    /// A list that isn't kept, its thread not started: read, and empty.
    pub(crate) fn unkept() -> Self {
        Recents {
            list: Vec::new(),
            zone: TimeZone::UTC,
            read: true,
            asks: None,
            told: None,
            wake: Wake::default(),
        }
    }

    /// Where the list is kept: `recent-documents` in the platform's app-data folder
    /// (`app_data::folder`: `~/Library/Application Support/Silico` on macOS,
    /// `$XDG_DATA_HOME/silico` or `~/.local/share/silico` on Linux); none if the platform
    /// names no home folder.
    pub fn default_file() -> Option<PathBuf> {
        crate::app_data::folder().map(|folder| folder.join(FILE_NAME))
    }

    /// The list, newest first, as the thread last told it.
    pub fn list(&self) -> &[Recent] {
        &self.list
    }

    /// Whether the file has been read (or found unreadable) yet.
    pub fn is_read(&self) -> bool {
        self.read
    }

    /// Whether the list is kept: false if its thread couldn't be started.
    pub fn is_kept(&self) -> bool {
        self.asks.is_some()
    }

    /// Whether `path` is in the list, as it was opened from it.
    pub fn holds(&self, path: &str) -> bool {
        self.list.iter().any(|recent| recent.path == path)
    }

    /// The document at `path` opened, or saved as: first in the list from now on.
    pub(crate) fn opened(&self, path: &str) {
        self.ask(Ask::Opened(path.to_owned()));
    }

    /// The open of the document at `path`, from the list, refused: dropped if nothing is
    /// there, which [`poll`](Self::poll) then tells.
    pub(crate) fn check(&self, path: &str) {
        self.ask(Ask::Check(path.to_owned()));
    }

    /// The document at `path` dropped from the list (a missing row's Remove from list), its
    /// file written without it; nothing if it isn't listed.
    pub(crate) fn remove(&self, path: &str) {
        self.ask(Ask::Remove(path.to_owned()));
    }

    /// Each document in the list looked for again, off the UI thread: the start page shown
    /// again, so its times (a document saved since) and its missing marks are now's.
    pub(crate) fn look(&self) {
        self.ask(Ask::Look);
    }

    fn ask(&self, ask: Ask) {
        if let Some(asks) = &self.asks {
            // A thread that has ended keeps nothing more; the list shown stays as it was.
            let _ = asks.send(ask);
        }
    }

    /// The platform's time zone, as the thread last read it: each document's time is said
    /// in it.
    pub fn zone(&self) -> &TimeZone {
        &self.zone
    }

    /// What the thread has told since the last poll, never waited for: the list taken, and
    /// the documents dropped because nothing was at their paths or as asked.
    pub(crate) fn poll(&mut self) -> Polled {
        let mut polled = Polled::default();
        let Some(told) = &self.told else {
            return polled;
        };
        for told in told.try_iter() {
            match told {
                Told::List(list, zone) => {
                    self.list = list;
                    self.zone = zone;
                    self.read = true;
                }
                Told::Gone(recent) => polled.gone.push(recent),
                Told::Removed(path) => polled.removed.push(path),
                Told::NotRemoved(path) => polled.not_removed.push(path),
            }
        }
        polled
    }

    /// The thread asks `ctx` for a frame whenever it tells something.
    pub(crate) fn waking(&self, ctx: &eframe::egui::Context) {
        self.wake.set(ctx);
    }
}

/// What was found at a document's path when it was last looked for.
#[derive(Debug, Clone)]
enum Found {
    /// Nothing (a link to nothing included).
    Nothing,
    /// Something, last modified then, if the file system said when.
    There(Option<Timestamp>),
}

/// What's at `path`: [`Found::Nothing`] only if nothing is there, not for a file that
/// can't be read, which is still there, with no time.
fn look(path: &str) -> Found {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Nothing,
        Err(_) => Found::There(None),
        Ok(metadata) => Found::There(
            metadata
                .modified()
                .ok()
                .and_then(|time| Timestamp::try_from(time).ok()),
        ),
    }
}

/// `paths` as the window lists them, each marked missing, or given its time, as `found`
/// has it (unmarked, with no time, if it hasn't been looked for).
fn listed(paths: &[String], found: &BTreeMap<String, Found>) -> Vec<Recent> {
    paths
        .iter()
        .map(|path| {
            let (missing, modified) = match found.get(path) {
                Some(Found::Nothing) => (true, None),
                Some(Found::There(modified)) => (false, *modified),
                None => (false, None),
            };
            Recent {
                path: path.clone(),
                missing,
                modified,
            }
        })
        .collect()
}

/// Each of `paths` looked for.
fn look_all(paths: &[String]) -> BTreeMap<String, Found> {
    paths
        .iter()
        .map(|path| (path.clone(), look(path)))
        .collect()
}

/// The thread that keeps the list. `file` is read once and the list told at once; then each
/// document in it is looked for, and the list told again with what was found (which are
/// missing, and when each was modified), so a folder that hangs (a network mount gone)
/// never hides the list. Then each ask is taken as it comes (those already waiting
/// together): the file read again, so what another instance of the app wrote meanwhile is
/// kept, the asks made to it, the file written if they changed it, and what changed told.
/// A document opened is there, and its time is read again; a [`Ask::Look`] looks for every
/// document again, once the asks with it have been made.
fn keep(file: &Path, read: Read, asked: &Receiver<Ask>, tell: &Sender<Told>, wake: &Wake) {
    let _span = tracing::info_span!("recent documents", file = %file.display()).entered();
    let mut paths: Vec<String> = read(file).map(|bytes| parse(&bytes)).unwrap_or_default();
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    // The platform's time zone, read here, on the list's thread (its rules may be read from
    // disk), with each look through the list.
    let mut zone = TimeZone::UTC;
    let mut told = (listed(&paths, &found), zone.clone());
    if tell
        .send(Told::List(told.0.clone(), told.1.clone()))
        .is_err()
    {
        return;
    }
    wake.wake();
    found = look_all(&paths);
    zone = TimeZone::system();
    let now = (listed(&paths, &found), zone.clone());
    if now != told {
        told = now;
        if tell
            .send(Told::List(told.0.clone(), told.1.clone()))
            .is_err()
        {
            return;
        }
        wake.wake();
    }
    while let Ok(first) = asked.recv() {
        let on_disk = read_capped(file, MAX_FILE_BYTES).map(|bytes| parse(&bytes));
        let before = on_disk.unwrap_or_else(|| paths.clone());
        let mut after = before.clone();
        let mut gone = Vec::new();
        let mut removed = Vec::new();
        let mut look_again = false;
        for ask in std::iter::once(first).chain(asked.try_iter()) {
            match ask {
                Ask::Opened(path) => {
                    if let Some(path) = absolute(&path) {
                        after.retain(|kept| *kept != path);
                        // Opened, so there: its time read again (a save as writes it).
                        let time = match look(&path) {
                            Found::There(time) => time,
                            Found::Nothing => None,
                        };
                        found.insert(path.clone(), Found::There(time));
                        after.insert(0, path);
                        after.truncate(KEPT);
                    }
                }
                Ask::Check(path) => {
                    if after.contains(&path) && nothing_at(&path) {
                        after.retain(|kept| *kept != path);
                        found.remove(&path);
                        gone.push(Recent {
                            path,
                            missing: true,
                            modified: None,
                        });
                    }
                }
                // What was found at it is dropped below, once the file is written without it.
                Ask::Remove(path) => {
                    after.retain(|kept| *kept != path);
                    removed.push(path);
                }
                Ask::Look => look_again = true,
            }
        }
        let mut written = true;
        if after != before {
            let lines: Vec<&str> = after.iter().map(String::as_str).collect();
            if let Err(e) = write(file, &text(&lines)) {
                tracing::warn!("the list of recent documents couldn't be written: {e}");
                written = false;
            }
        }
        // A removal the file doesn't hold isn't one: the list stays as the file has it, so a
        // row said removed never comes back at the next read.
        paths = if written || removed.is_empty() {
            after
        } else {
            before
        };
        if look_again {
            found = look_all(&paths);
            zone = TimeZone::system();
        } else {
            found.retain(|path, _| paths.contains(path));
        }
        let now = (listed(&paths, &found), zone.clone());
        if now != told {
            told = now;
            if tell
                .send(Told::List(told.0.clone(), told.1.clone()))
                .is_err()
            {
                return;
            }
        }
        for recent in gone {
            if tell.send(Told::Gone(recent)).is_err() {
                return;
            }
        }
        // Told after the list without them, so the window has that list when it hears; or,
        // the file unwritten, that they stay.
        for path in removed {
            let told = if written {
                Told::Removed(path)
            } else {
                Told::NotRemoved(path)
            };
            if tell.send(told).is_err() {
                return;
            }
        }
        wake.wake();
    }
}

/// Whether nothing is at `path` (a link to nothing included): not a file that can't be read,
/// which is still there.
fn nothing_at(path: &str) -> bool {
    matches!(std::fs::metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// `path` made absolute against the working directory, if it can be kept: see [`keepable`].
fn absolute(path: &str) -> Option<String> {
    let path = std::path::absolute(path).ok()?;
    let path = path.into_os_string().into_string().ok()?;
    keepable(&path).then_some(path)
}

/// Whether `path` can be a line of the file: absolute, at most [`MAX_PATH_BYTES`] long, and
/// with no line break or NUL in it.
fn keepable(path: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && Path::new(path).is_absolute()
        && !path.contains(['\n', '\r', '\0'])
}

/// The paths of the file `bytes`: none unless it's UTF-8, its first line is [`HEADER`], and
/// each line after it, up to the [`KEPT`]th path, a path [`keepable`] (the last line's break
/// optional); a path already listed is listed once. The lines after the [`KEPT`]th path
/// aren't read.
pub(crate) fn parse(bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let Some(body) = text
        .strip_prefix(HEADER)
        .and_then(|rest| rest.strip_prefix('\n'))
    else {
        return Vec::new();
    };
    let mut paths: Vec<String> = Vec::new();
    for line in body.split_terminator('\n') {
        if paths.len() == KEPT {
            break;
        }
        if !keepable(line) {
            return Vec::new();
        }
        if !paths.iter().any(|path| path == line) {
            paths.push(line.to_owned());
        }
    }
    paths
}

/// The file's text for `paths`, newest first: [`HEADER`], then each path on a line.
pub(crate) fn text(paths: &[&str]) -> String {
    let mut text = String::from(HEADER);
    text.push('\n');
    for path in paths {
        text.push_str(path);
        text.push('\n');
    }
    text
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn a_list_is_read_as_written() {
        let paths = ["/a/one.silico", "/b/two.silico"];
        assert_eq!(
            text(&paths),
            "silico recent-documents 1\n/a/one.silico\n/b/two.silico\n"
        );
        assert_eq!(parse(text(&paths).as_bytes()), paths);
        // The last line's break is optional.
        assert_eq!(
            parse(b"silico recent-documents 1\n/a/one.silico"),
            ["/a/one.silico"]
        );
        assert!(parse(text(&[]).as_bytes()).is_empty());
    }

    #[test]
    fn anything_else_is_an_empty_list() {
        for bytes in [
            &b""[..],
            b"\xff\xfe",
            b"silico recent-documents 1",
            b"silico recent-documents 2\n/a/one.silico\n",
            b"/a/one.silico\n",
            b" silico recent-documents 1\n/a/one.silico\n",
            b"silico recent-documents 1\r\n/a/one.silico\n",
            // A relative path, an empty line, a carriage return, a NUL: each spoils it all.
            b"silico recent-documents 1\n/a/one.silico\nrelative.silico\n",
            b"silico recent-documents 1\n/a/one.silico\n\n/b/two.silico\n",
            b"silico recent-documents 1\n/a/one.silico\r\n",
            b"silico recent-documents 1\n/a/o\0ne.silico\n",
            b"silico recent-documents 1\n/a/\xffone.silico\n",
        ] {
            assert!(
                parse(bytes).is_empty(),
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn a_path_listed_twice_is_listed_once_and_only_the_first_ten_are_kept() {
        let mut lines = String::from("silico recent-documents 1\n/a/0.silico\n/a/0.silico\n");
        for n in 1..=11 {
            lines.push_str(&format!("/a/{n}.silico\n"));
        }
        let want: Vec<String> = (0..10).map(|n| format!("/a/{n}.silico")).collect();
        assert_eq!(parse(lines.as_bytes()), want);
    }

    #[test]
    fn only_an_absolute_path_with_no_break_or_nul_is_kept() {
        assert!(keepable("/a/one.silico"));
        for path in ["", "one.silico", "/a/\none", "/a/\rone", "/a/\0one"] {
            assert!(!keepable(path), "{path:?}");
        }
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            absolute("one.silico"),
            here.join("one.silico").to_str().map(str::to_owned)
        );
        assert_eq!(absolute("/a/one.silico\n"), None);
    }

    /// What the list's thread tells, polled until `done` holds.
    fn polled_until(recents: &mut Recents, done: impl Fn(&Recents) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !done(recents) {
            recents.poll();
            assert!(std::time::Instant::now() < deadline, "never told");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn the_thread_keeps_the_ten_opened_last_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deep").join(FILE_NAME);
        let mut recents = Recents::at(file.clone());
        polled_until(&mut recents, Recents::is_read);
        assert!(recents.list().is_empty());
        let paths: Vec<String> = (0..12).map(|n| format!("/a/{n}.silico")).collect();
        for path in &paths {
            recents.opened(path);
        }
        polled_until(&mut recents, |recents| {
            recents.list().first().map(|recent| recent.path.as_str()) == Some("/a/11.silico")
        });
        let want: Vec<&str> = (2..12).rev().map(|n| paths[n].as_str()).collect();
        let listed: Vec<&str> = recents
            .list()
            .iter()
            .map(|recent| recent.path.as_str())
            .collect();
        assert_eq!(listed, want);
        assert!(recents.list().iter().all(|recent| !recent.missing));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text(&want));
    }

    /// `parse(bytes)`, on a thread of its own, within 30 seconds: a parse that grows with the
    /// square of the lines it reads would take hours on the inputs below.
    fn parsed_promptly(bytes: Vec<u8>) -> Vec<String> {
        let (tell, told) = std::sync::mpsc::channel();
        std::thread::spawn(move || tell.send(parse(&bytes)));
        told.recv_timeout(std::time::Duration::from_secs(30))
            .expect("parse took too long")
    }

    #[test]
    fn a_path_longer_than_4096_bytes_spoils_the_file() {
        let longest = format!("/{}", "a".repeat(MAX_PATH_BYTES - 1));
        assert_eq!(
            parse(text(&[longest.as_str()]).as_bytes()),
            std::slice::from_ref(&longest)
        );
        let longer = format!("{longest}a");
        assert!(parse(text(&[longer.as_str()]).as_bytes()).is_empty());
        assert!(!keepable(&longer));
        // A 10 MB line, read whole: nothing, at once.
        let huge = format!("/{}", "a".repeat(10_000_000));
        assert!(parsed_promptly(text(&[huge.as_str()]).into_bytes()).is_empty());
    }

    #[test]
    fn a_million_paths_are_read_only_as_far_as_the_tenth() {
        let mut lines = String::from("silico recent-documents 1\n");
        for n in 0..1_000_000 {
            lines.push_str(&format!("/a/{n}.silico\n"));
        }
        let want: Vec<String> = (0..KEPT).map(|n| format!("/a/{n}.silico")).collect();
        assert_eq!(parsed_promptly(lines.into_bytes()), want);
    }

    #[test]
    fn only_the_files_first_64_kib_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        // One path listed again and again past the cap, then another.
        let mut lines = String::from("silico recent-documents 1\n");
        while lines.len() <= MAX_FILE_BYTES {
            lines.push_str("/a/0.silico\n");
        }
        lines.push_str("/a/1.silico\n");
        std::fs::write(&file, &lines).unwrap();
        assert_eq!(parse(lines.as_bytes()), ["/a/0.silico", "/a/1.silico"]);
        let read = read_capped(&file, MAX_FILE_BYTES).unwrap();
        assert!(read.len() <= MAX_FILE_BYTES);
        assert!(read.ends_with(b"\n"));
        assert_eq!(parse(&read), ["/a/0.silico"]);
        // A 10 MB line in the file: the header alone is read, and lists nothing.
        let huge = format!("/{}", "a".repeat(10_000_000));
        std::fs::write(&file, text(&[huge.as_str()])).unwrap();
        assert_eq!(
            read_capped(&file, MAX_FILE_BYTES).unwrap(),
            format!("{HEADER}\n").into_bytes()
        );
        // Something that isn't a regular file isn't read.
        assert_eq!(read_capped(dir.path(), MAX_FILE_BYTES), None);
        assert_eq!(read_capped(&dir.path().join("none"), MAX_FILE_BYTES), None);
    }

    #[test]
    fn a_huge_file_is_listed_at_once_as_far_as_its_tenth_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        let mut lines = String::from("silico recent-documents 1\n");
        for n in 0..1_000_000 {
            lines.push_str(&format!("/a/{n}.silico\n"));
        }
        std::fs::write(&file, lines).unwrap();
        let mut recents = Recents::at(file);
        polled_until(&mut recents, Recents::is_read);
        let listed: Vec<String> = recents
            .list()
            .iter()
            .map(|recent| recent.path.clone())
            .collect();
        let want: Vec<String> = (0..KEPT).map(|n| format!("/a/{n}.silico")).collect();
        assert_eq!(listed, want);
    }

    #[test]
    fn two_instances_on_one_file_keep_each_others_documents() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data").join(FILE_NAME);
        let mut first = Recents::at(file.clone());
        let mut second = Recents::at(file.clone());
        polled_until(&mut first, Recents::is_read);
        polled_until(&mut second, Recents::is_read);
        let opened = |recents: &mut Recents, path: &str| {
            recents.opened(path);
            polled_until(recents, |recents| {
                recents.list().first().map(|recent| recent.path.as_str()) == Some(path)
            });
        };
        opened(&mut first, "/a/first-1.silico");
        opened(&mut second, "/a/second-1.silico");
        opened(&mut first, "/a/first-2.silico");
        let all = [
            "/a/first-2.silico",
            "/a/second-1.silico",
            "/a/first-1.silico",
        ];
        let listed: Vec<&str> = first
            .list()
            .iter()
            .map(|recent| recent.path.as_str())
            .collect();
        assert_eq!(listed, all);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text(&all));
        // No file but the list's is left in its folder.
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE_NAME]);
    }

    #[test]
    fn the_thread_asks_the_window_for_a_frame_when_it_tells_the_list() {
        let ctx = eframe::egui::Context::default();
        let asked = std::sync::Arc::new(AtomicU64::new(0));
        let counted = std::sync::Arc::clone(&asked);
        ctx.set_request_repaint_callback(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let dir = tempfile::tempdir().unwrap();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let read: Read = Box::new(move |_| {
            released.recv().ok()?;
            Some(text(&["/a/one.silico"]).into_bytes())
        });
        let mut recents = Recents::reading(dir.path().join(FILE_NAME), read);
        recents.waking(&ctx);
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while asked.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "no frame asked for");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        polled_until(&mut recents, Recents::is_read);
        assert_eq!(recents.list()[0].path, "/a/one.silico");
    }

    #[test]
    fn ten_of_the_longest_paths_are_read_whole_and_a_file_of_exactly_64_kib_too() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        // Ten paths of 4096 bytes: 26 + 10 · 4097 = 40,996 bytes, under 64 KiB (65,536).
        let longest: Vec<String> = (0..10)
            .map(|n| format!("/{n}{}", "a".repeat(4094)))
            .collect();
        let lines: Vec<&str> = longest.iter().map(String::as_str).collect();
        let whole = text(&lines);
        assert_eq!(whole.len(), 40_996);
        std::fs::write(&file, &whole).unwrap();
        assert_eq!(
            read_capped(&file, MAX_FILE_BYTES).unwrap(),
            whole.as_bytes()
        );
        assert_eq!(parse(&read_capped(&file, MAX_FILE_BYTES).unwrap()), longest);
        // Exactly 65,536 bytes, its last line unbroken: all of it read, that line included.
        let mut exact = String::from("silico recent-documents 1\n/a/0.silico\n/");
        exact.push_str(&"b".repeat(65_536 - exact.len()));
        assert_eq!(exact.len(), 65_536);
        std::fs::write(&file, &exact).unwrap();
        assert_eq!(
            read_capped(&file, MAX_FILE_BYTES).unwrap(),
            exact.as_bytes()
        );
        // One byte more: the line the cap cuts is dropped.
        exact.push('b');
        std::fs::write(&file, &exact).unwrap();
        assert_eq!(
            read_capped(&file, MAX_FILE_BYTES).unwrap(),
            b"silico recent-documents 1\n/a/0.silico\n"
        );
    }

    #[test]
    fn a_list_holds_only_the_paths_in_it_and_one_not_kept_says_so() {
        let read: Read = Box::new(|_| Some(text(&["/a/one.silico", "/b/two.silico"]).into_bytes()));
        let dir = tempfile::tempdir().unwrap();
        let mut recents = Recents::reading(dir.path().join(FILE_NAME), read);
        polled_until(&mut recents, Recents::is_read);
        assert!(recents.is_kept());
        assert!(recents.holds("/a/one.silico"));
        assert!(recents.holds("/b/two.silico"));
        for other in ["/a/three.silico", "/a/one", "one.silico", ""] {
            assert!(!recents.holds(other), "{other}");
        }
        let unkept = Recents::unkept();
        assert!(!unkept.is_kept());
        assert!(unkept.is_read());
        assert!(unkept.list().is_empty());
        assert!(!unkept.holds("/a/one.silico"));
    }

    #[test]
    fn the_list_is_kept_in_the_platforms_app_data_folder() {
        // directories' folders, as recent.rs's documentation gives them.
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let folder = if cfg!(target_os = "macos") {
            home.join("Library")
                .join("Application Support")
                .join("Silico")
        } else if cfg!(target_os = "linux") {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|data| data.is_absolute())
                .unwrap_or_else(|| home.join(".local").join("share"))
                .join("silico")
        } else {
            return;
        };
        assert_eq!(
            Recents::default_file(),
            Some(folder.join("recent-documents"))
        );
    }

    #[test]
    fn a_recent_is_named_by_its_file_and_its_folder() {
        let recent = Recent {
            path: "/a/b/one.silico".to_owned(),
            missing: false,
            modified: None,
        };
        assert_eq!((recent.name(), recent.folder()), ("one.silico", "/a/b"));
    }

    /// The file at `path` given `time` as its modified time, as a save gives it one (std's
    /// `File::set_modified`, as the core's cache tests set theirs).
    pub(crate) fn modified_at(path: &Path, time: Timestamp) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::from(time))
            .unwrap();
    }

    /// `seconds` after the epoch.
    fn second(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    /// Each listed document's path, whether it's missing, and its time in seconds after the
    /// epoch, if it has one.
    fn seen(recents: &Recents) -> Vec<(String, bool, Option<i64>)> {
        recents
            .list()
            .iter()
            .map(|recent| {
                (
                    recent.path.clone(),
                    recent.missing,
                    recent.modified.map(|time| time.as_second()),
                )
            })
            .collect()
    }

    #[test]
    fn each_document_is_given_its_files_modified_time_and_looked_for_again_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (dir.path().join("one.silico"), dir.path().join("two.silico"));
        let gone = dir.path().join("gone.silico");
        for (path, seconds) in [(&one, 1_000_000_000), (&two, 1_700_000_000)] {
            std::fs::write(path, b"").unwrap();
            modified_at(path, second(seconds));
        }
        let paths: Vec<String> = [&one, &two, &gone]
            .iter()
            .map(|path| path.to_str().unwrap().to_owned())
            .collect();
        let listed: Vec<&str> = paths.iter().map(String::as_str).collect();
        let file = dir.path().join(FILE_NAME);
        std::fs::write(&file, text(&listed)).unwrap();
        let mut recents = Recents::at(file.clone());
        let want = vec![
            (paths[0].clone(), false, Some(1_000_000_000)),
            (paths[1].clone(), false, Some(1_700_000_000)),
            (paths[2].clone(), true, None),
        ];
        polled_until(&mut recents, |recents| seen(recents) == want);
        // Said in the platform's zone, read with the list.
        assert_eq!(recents.zone(), &TimeZone::system());
        // The files change on disk: nothing is told until the list is looked through again.
        modified_at(&one, second(1_800_000_000));
        std::fs::write(&gone, b"").unwrap();
        modified_at(&gone, second(1_600_000_000));
        std::fs::remove_file(&two).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        recents.poll();
        assert_eq!(seen(&recents), want);
        recents.look();
        let looked = vec![
            (paths[0].clone(), false, Some(1_800_000_000)),
            (paths[1].clone(), true, None),
            (paths[2].clone(), false, Some(1_600_000_000)),
        ];
        polled_until(&mut recents, |recents| seen(recents) == looked);
        // Looking writes nothing: the list's file is as it was.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text(&listed));
        // A document opened is there, with its time read as it's opened.
        modified_at(&one, second(1_900_000_000));
        recents.opened(&paths[0]);
        let opened = vec![
            (paths[0].clone(), false, Some(1_900_000_000)),
            (paths[1].clone(), true, None),
            (paths[2].clone(), false, Some(1_600_000_000)),
        ];
        polled_until(&mut recents, |recents| seen(recents) == opened);
    }

    #[test]
    fn a_document_removed_leaves_the_list_and_its_file_whether_or_not_it_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept.silico");
        std::fs::write(&kept, b"").unwrap();
        let kept = kept.to_str().unwrap().to_owned();
        let gone = dir.path().join("gone.silico").to_str().unwrap().to_owned();
        let file = dir.path().join("data").join(FILE_NAME);
        let mut recents = Recents::at(file.clone());
        polled_until(&mut recents, Recents::is_read);
        recents.opened(&kept);
        recents.opened(&gone);
        polled_until(&mut recents, |recents| recents.list().len() == 2);
        // A path not listed: nothing changes.
        recents.remove("/a/other.silico");
        recents.remove(&gone);
        polled_until(&mut recents, |recents| {
            recents
                .list()
                .iter()
                .map(|recent| recent.path.as_str())
                .collect::<Vec<_>>()
                == [kept.as_str()]
        });
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            text(&[kept.as_str()])
        );
        // One that's there is removed as well; and the list read again lists nothing.
        recents.remove(&kept);
        polled_until(&mut recents, |recents| recents.list().is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text(&[]));
        drop(recents);
        let mut again = Recents::at(file);
        polled_until(&mut again, Recents::is_read);
        assert!(again.list().is_empty());
    }
}
