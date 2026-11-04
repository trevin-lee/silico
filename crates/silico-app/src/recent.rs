//! The documents opened lately: the start page's Recent (docs/design/m5-ui/DESIGN.md,
//! *Start page or blank canvas?*), kept in a file of the app's own in the platform's
//! app-data folder ([`Recents::default_file`]).
//!
//! The file is read, and each document in it looked for, on a thread of its own, which
//! reads it again and writes it after each change, so the UI thread never waits on it
//! (invariant 4), and what another instance of the app wrote meanwhile is kept: the window
//! asks the thread ([`Recents::opened`], [`Recents::check`]) and draws the list it last told
//! ([`Recents::list`]). Its form is a header line and an absolute path a line, newest first
//! (`text`); a file that isn't one (`parse`), or can't be read, is an empty list, never a
//! failure, and at most [`MAX_FILE_BYTES`] of it is read.

use std::collections::BTreeSet;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

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
}

impl Recent {
    /// Its file's name, as its tab names it.
    pub fn name(&self) -> &str {
        Path::new(&self.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.path)
    }

    /// The folder it's in.
    pub fn folder(&self) -> &str {
        Path::new(&self.path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("")
    }
}

/// What the window asks the list's thread.
enum Ask {
    /// A document opened (or saved as), at this path as it was given: first in the list now.
    Opened(String),
    /// A document whose open was refused: dropped from the list if nothing is at its path.
    Check(String),
}

/// What the list's thread tells the window.
enum Told {
    /// The list, as it is now.
    List(Vec<Recent>),
    /// A document dropped from the list because nothing was at its path.
    Gone(Recent),
}

/// What reads the list's file the first time: its bytes, or none if it can't be read (a
/// test's, to hold the read). Each later read, before a write, is [`read_capped`]'s.
pub type Read = Box<dyn FnOnce(&Path) -> Option<Vec<u8>> + Send>;

/// The list of recent documents, as the window last heard it from the thread that keeps it.
pub struct Recents {
    list: Vec<Recent>,
    read: bool,
    asks: Option<Sender<Ask>>,
    told: Option<Receiver<Told>>,
    wake: Wake,
}

impl Recents {
    /// The list kept in `file`, read on a thread of its own ([`read_capped`]: a regular
    /// file, never a pipe that could hang it, and at most [`MAX_FILE_BYTES`] of it).
    pub fn at(file: PathBuf) -> Self {
        Self::reading(file, Box::new(read_capped))
    }

    /// The list kept in `file`, read first by `read` on a thread of its own.
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
            read: true,
            asks: None,
            told: None,
            wake: Wake::default(),
        }
    }

    /// Where the list is kept: `recent-documents` in the platform's app-data folder
    /// (`~/Library/Application Support/Silico` on macOS, `$XDG_DATA_HOME/silico` or
    /// `~/.local/share/silico` on Linux); none if the platform names no home folder.
    pub fn default_file() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "Silico").map(|dirs| dirs.data_dir().join(FILE_NAME))
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

    fn ask(&self, ask: Ask) {
        if let Some(asks) = &self.asks {
            // A thread that has ended keeps nothing more; the list shown stays as it was.
            let _ = asks.send(ask);
        }
    }

    /// What the thread has told since the last poll, never waited for: the list taken, and
    /// the documents dropped because nothing was at their paths.
    pub(crate) fn poll(&mut self) -> Vec<Recent> {
        let mut gone = Vec::new();
        let Some(told) = &self.told else {
            return gone;
        };
        for told in told.try_iter() {
            match told {
                Told::List(list) => {
                    self.list = list;
                    self.read = true;
                }
                Told::Gone(recent) => gone.push(recent),
            }
        }
        gone
    }

    /// The thread asks `ctx` for a frame whenever it tells something.
    pub(crate) fn waking(&self, ctx: &eframe::egui::Context) {
        self.wake.set(ctx);
    }
}

/// `paths` as the window lists them, each marked if it's among `missing`.
fn listed(paths: &[String], missing: &BTreeSet<String>) -> Vec<Recent> {
    paths
        .iter()
        .map(|path| Recent {
            path: path.clone(),
            missing: missing.contains(path),
        })
        .collect()
}

/// The thread that keeps the list. `file` is read once and the list told at once; then each
/// document in it is looked for, and the list told again if any is missing, so a folder
/// that hangs (a network mount gone) never hides the list. Then each ask is taken as it
/// comes (those already waiting together): the file read again, so what another instance
/// of the app wrote meanwhile is kept, the asks made to it, the file written if they
/// changed it, and what changed told.
fn keep(file: &Path, read: Read, asked: &Receiver<Ask>, tell: &Sender<Told>, wake: &Wake) {
    let _span = tracing::info_span!("recent documents", file = %file.display()).entered();
    let mut paths: Vec<String> = read(file).map(|bytes| parse(&bytes)).unwrap_or_default();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut told = listed(&paths, &missing);
    if tell.send(Told::List(told.clone())).is_err() {
        return;
    }
    wake.wake();
    missing = paths
        .iter()
        .filter(|path| nothing_at(path))
        .cloned()
        .collect();
    if !missing.is_empty() {
        told = listed(&paths, &missing);
        if tell.send(Told::List(told.clone())).is_err() {
            return;
        }
        wake.wake();
    }
    while let Ok(first) = asked.recv() {
        let on_disk = read_capped(file).map(|bytes| parse(&bytes));
        let before = on_disk.unwrap_or_else(|| paths.clone());
        let mut after = before.clone();
        let mut gone = Vec::new();
        for ask in std::iter::once(first).chain(asked.try_iter()) {
            match ask {
                Ask::Opened(path) => {
                    if let Some(path) = absolute(&path) {
                        after.retain(|kept| *kept != path);
                        missing.remove(&path);
                        after.insert(0, path);
                        after.truncate(KEPT);
                    }
                }
                Ask::Check(path) => {
                    if after.contains(&path) && nothing_at(&path) {
                        after.retain(|kept| *kept != path);
                        missing.remove(&path);
                        gone.push(Recent {
                            path,
                            missing: true,
                        });
                    }
                }
            }
        }
        if after != before {
            let lines: Vec<&str> = after.iter().map(String::as_str).collect();
            if let Err(e) = write(file, &lines) {
                tracing::warn!("the list of recent documents couldn't be written: {e}");
            }
        }
        paths = after;
        missing.retain(|path| paths.contains(path));
        let now = listed(&paths, &missing);
        if now != told {
            told = now;
            if tell.send(Told::List(told.clone())).is_err() {
                return;
            }
        }
        for recent in gone {
            if tell.send(Told::Gone(recent)).is_err() {
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

/// `file`'s first [`MAX_FILE_BYTES`], if it's a regular file that can be read (a pipe, which
/// could hang the read, isn't); if there's more, the line the cap cuts is dropped too.
pub(crate) fn read_capped(file: &Path) -> Option<Vec<u8>> {
    if !std::fs::metadata(file).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .ok()?
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_FILE_BYTES {
        bytes.truncate(MAX_FILE_BYTES);
        let whole = bytes
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        bytes.truncate(whole);
    }
    Some(bytes)
}

/// The number of the next file written beside the list: with the process's ID, a name no
/// other write, in this instance of the app or another, uses at once.
static WRITES: AtomicU64 = AtomicU64::new(0);

/// `paths` written to `file`, its folder made if it isn't there: to a file of its own beside
/// it first, flushed to the disk, then moved over it, so a reader never sees half a list;
/// that file removed if the write fails.
fn write(file: &Path, paths: &[&str]) -> std::io::Result<()> {
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
        new.write_all(text(paths).as_bytes())?;
        new.sync_all()?;
        std::fs::rename(&beside, file)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    written
}

#[cfg(test)]
mod tests {
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
        let read = read_capped(&file).unwrap();
        assert!(read.len() <= MAX_FILE_BYTES);
        assert!(read.ends_with(b"\n"));
        assert_eq!(parse(&read), ["/a/0.silico"]);
        // A 10 MB line in the file: the header alone is read, and lists nothing.
        let huge = format!("/{}", "a".repeat(10_000_000));
        std::fs::write(&file, text(&[huge.as_str()])).unwrap();
        assert_eq!(
            read_capped(&file).unwrap(),
            format!("{HEADER}\n").into_bytes()
        );
        // Something that isn't a regular file isn't read.
        assert_eq!(read_capped(dir.path()), None);
        assert_eq!(read_capped(&dir.path().join("none")), None);
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
        assert_eq!(read_capped(&file).unwrap(), whole.as_bytes());
        assert_eq!(parse(&read_capped(&file).unwrap()), longest);
        // Exactly 65,536 bytes, its last line unbroken: all of it read, that line included.
        let mut exact = String::from("silico recent-documents 1\n/a/0.silico\n/");
        exact.push_str(&"b".repeat(65_536 - exact.len()));
        assert_eq!(exact.len(), 65_536);
        std::fs::write(&file, &exact).unwrap();
        assert_eq!(read_capped(&file).unwrap(), exact.as_bytes());
        // One byte more: the line the cap cuts is dropped.
        exact.push('b');
        std::fs::write(&file, &exact).unwrap();
        assert_eq!(
            read_capped(&file).unwrap(),
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
        };
        assert_eq!((recent.name(), recent.folder()), ("one.silico", "/a/b"));
    }
}
