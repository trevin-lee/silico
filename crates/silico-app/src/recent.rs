//! The documents opened lately: the start page's Recent (docs/design/m5-ui/DESIGN.md,
//! *Start page or blank canvas?*), kept in a file of the app's own in the platform's
//! app-data folder ([`Recents::default_file`]).
//!
//! The file is read, and each document in it looked for, on a thread of its own, which
//! writes it again after each change, so the UI thread never waits on it (invariant 4): the
//! window asks the thread ([`Recents::opened`], [`Recents::check`]) and draws the list it
//! last told ([`Recents::list`]). Its form is a header line and an absolute path a line,
//! newest first ([`text`]); a file that isn't one ([`parse`]), or can't be read, is an empty
//! list, never a failure.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::start::Wake;

/// How many documents the list keeps.
pub const KEPT: usize = 10;

/// The list's file's name in the app-data folder.
pub const FILE_NAME: &str = "recent-documents";

/// The file's first line: its form, and the form's version.
pub const HEADER: &str = "silico recent-documents 1";

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
    /// A document opened, at this path as it was opened: first in the list now.
    Opened(String),
    /// A document whose open was refused: dropped from the list if nothing is at its path.
    Check(String),
}

/// What the list's thread tells the window.
enum Told {
    /// The list, as it is now.
    List(Vec<Recent>),
    /// A document dropped from the list because nothing was at its path.
    Gone(String),
}

/// What reads the list's file: its bytes, or none if it can't be read (a test's, to hold
/// the read).
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
    /// The list kept in `file`, read on a thread of its own as a document is
    /// (`silico_core::file::read`: a regular file, never a pipe that could hang it).
    pub fn at(file: PathBuf) -> Self {
        Self::reading(file, Box::new(|file| silico_core::file::read(file).ok()))
    }

    /// The list kept in `file`, read by `read` on a thread of its own.
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
                Recents {
                    list: Vec::new(),
                    read: true,
                    asks: None,
                    told: None,
                    wake,
                }
            }
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

    /// Whether `path` is in the list, as it was opened from it.
    pub fn holds(&self, path: &str) -> bool {
        self.list.iter().any(|recent| recent.path == path)
    }

    /// The document at `path` opened: first in the list from now on.
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
    /// the paths dropped because nothing was at them.
    pub(crate) fn poll(&mut self) -> Vec<String> {
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
                Told::Gone(path) => gone.push(path),
            }
        }
        gone
    }

    /// The thread asks `ctx` for a frame whenever it tells something.
    pub(crate) fn waking(&self, ctx: &eframe::egui::Context) {
        self.wake.set(ctx);
    }
}

/// The thread that keeps the list: `file` read once, each document in it looked for, then
/// each ask taken as it comes (those already waiting together), the file written after each
/// that changed the list, and what changed told.
fn keep(file: &Path, read: Read, asked: &Receiver<Ask>, tell: &Sender<Told>, wake: &Wake) {
    let _span = tracing::info_span!("recent documents", file = %file.display()).entered();
    let mut list: Vec<Recent> = read(file)
        .map(|bytes| parse(&bytes))
        .unwrap_or_default()
        .into_iter()
        .map(|path| Recent {
            missing: nothing_at(&path),
            path,
        })
        .collect();
    if tell.send(Told::List(list.clone())).is_err() {
        return;
    }
    wake.wake();
    while let Ok(first) = asked.recv() {
        let before = list.clone();
        let mut gone = Vec::new();
        for ask in std::iter::once(first).chain(asked.try_iter()) {
            match ask {
                Ask::Opened(path) => {
                    if let Some(path) = absolute(&path) {
                        list.retain(|recent| recent.path != path);
                        list.insert(
                            0,
                            Recent {
                                path,
                                missing: false,
                            },
                        );
                        list.truncate(KEPT);
                    }
                }
                Ask::Check(path) => {
                    if list.iter().any(|recent| recent.path == path) && nothing_at(&path) {
                        list.retain(|recent| recent.path != path);
                        gone.push(path);
                    }
                }
            }
        }
        if list != before {
            let paths: Vec<&str> = list.iter().map(|recent| recent.path.as_str()).collect();
            if let Err(e) = write(file, &paths) {
                tracing::warn!("the list of recent documents couldn't be written: {e}");
            }
            if tell.send(Told::List(list.clone())).is_err() {
                return;
            }
        }
        for path in gone {
            if tell.send(Told::Gone(path)).is_err() {
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

/// Whether `path` can be a line of the file: absolute, and with no line break or NUL in it.
fn keepable(path: &str) -> bool {
    Path::new(path).is_absolute() && !path.contains(['\n', '\r', '\0'])
}

/// The paths of the file `bytes`: none unless it's UTF-8, its first line is [`HEADER`], and
/// each line after it a path [`keepable`] (the last line's break optional); a path already
/// listed is listed once, and those past the first [`KEPT`] dropped.
pub fn parse(bytes: &[u8]) -> Vec<String> {
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
        if !keepable(line) {
            return Vec::new();
        }
        if !paths.iter().any(|path| path == line) {
            paths.push(line.to_owned());
        }
    }
    paths.truncate(KEPT);
    paths
}

/// The file's text for `paths`, newest first: [`HEADER`], then each path on a line.
pub fn text(paths: &[&str]) -> String {
    let mut text = String::from(HEADER);
    text.push('\n');
    for path in paths {
        text.push_str(path);
        text.push('\n');
    }
    text
}

/// `paths` written to `file`, its folder made if it isn't there: to a file beside it first,
/// then moved over it, so a reader never sees half a list.
fn write(file: &Path, paths: &[&str]) -> std::io::Result<()> {
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut beside = file.as_os_str().to_owned();
    beside.push(".new");
    let beside = PathBuf::from(beside);
    std::fs::write(&beside, text(paths))?;
    std::fs::rename(&beside, file)
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

    #[test]
    fn a_recent_is_named_by_its_file_and_its_folder() {
        let recent = Recent {
            path: "/a/b/one.silico".to_owned(),
            missing: false,
        };
        assert_eq!((recent.name(), recent.folder()), ("one.silico", "/a/b"));
    }
}
