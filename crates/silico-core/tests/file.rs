//! M2.9: a document's file saved safely (`silico_core::file`), the cases a client meets
//! without timing: a file changed since it was read is left as it is, and the error says
//! to read it again.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::ErrorCode;

#[test]
fn a_save_over_a_file_changed_since_it_was_read_is_refused_and_says_to_read_it_again() {
    let dir = std::env::temp_dir().join(format!("silico-save-changed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("part.silico");
    std::fs::write(&file, b"edited").unwrap();
    let error = silico_core::file::save(&file, b"as read", b"formatted").unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoFailed);
    assert!(error.to_string().contains("read it again"), "{error}");
    assert_eq!(std::fs::read(&file).unwrap(), b"edited");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_save_replaces_the_file_whole_rather_than_writing_into_it() {
    // Found by the milestone's tests review: a copy over the file, then the partial file
    // removed, passed every test; a crash mid-copy would leave the document cut short. A
    // save renames a new file over the old: another inode.
    use std::os::unix::fs::MetadataExt;
    let scratch = Scratch::new("replaced");
    let file = scratch.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    let before = std::fs::metadata(&file).unwrap().ino();
    silico_core::file::save(&file, b"as read", b"formatted").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"formatted");
    assert_ne!(std::fs::metadata(&file).unwrap().ino(), before);
}

/// A directory of its own for one test, removed after.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("silico-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_document_of_exactly_the_most_a_reader_takes_is_read_whole_and_one_more_byte_shows() {
    // `read` reads up to one byte past the most a document may be, so the reader sees a
    // document of exactly the most whole, and one byte longer as too long (found by the
    // mutation run: reading one byte fewer passed every test).
    let most = silico_core::document::json::MAX_INPUT_LEN;
    let dir = Scratch::new("most");
    let file = dir.0.join("part.silico");
    let f = std::fs::File::create(&file).unwrap();
    f.set_len(most as u64).unwrap();
    assert_eq!(silico_core::file::read(&file).unwrap().len(), most);
    f.set_len(most as u64 + 10).unwrap();
    assert_eq!(silico_core::file::read(&file).unwrap().len(), most + 1);
}

#[test]
fn a_save_takes_the_next_name_beside_the_file_when_one_is_left_from_a_killed_run() {
    // A run killed with this process's number left `.part.silico.fmt-PID`: the save uses
    // `-1`, and leaves the other as it was (found by the mutation run: untested).
    let dir = Scratch::new("left");
    let file = dir.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    let left = format!(".part.silico.fmt-{}", std::process::id());
    std::fs::write(dir.0.join(&left), b"left").unwrap();
    silico_core::file::save(&file, b"as read", b"formatted").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"formatted");
    assert_eq!(dir.names(), [left.clone(), "part.silico".to_owned()]);
    assert_eq!(std::fs::read(dir.0.join(&left)).unwrap(), b"left");
}

#[test]
fn a_save_with_every_name_beside_the_file_taken_says_so_and_writes_nothing() {
    let dir = Scratch::new("taken");
    let file = dir.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    let pid = std::process::id();
    std::fs::write(dir.0.join(format!(".part.silico.fmt-{pid}")), b"").unwrap();
    for n in 1..64 {
        std::fs::write(dir.0.join(format!(".part.silico.fmt-{pid}-{n}")), b"").unwrap();
    }
    let error = silico_core::file::save(&file, b"as read", b"formatted").unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoFailed);
    // All 64 names tried, and the message names them: the first, and -1 to -63.
    let said = format!(
        "every name for a file beside it is taken (\"{}/.part.silico.fmt-{pid}\", and -1 to -63)",
        // As the system names the directory, through any link (the temporary directory's),
        // quoted, as every path an error says is.
        std::fs::canonicalize(&dir.0).unwrap().display()
    );
    assert!(error.to_string().contains(&said), "{error}");
    assert_eq!(std::fs::read(&file).unwrap(), b"as read");
    assert_eq!(dir.names().len(), 65);
    // With the last name free, it's used: every name is tried.
    std::fs::remove_file(dir.0.join(format!(".part.silico.fmt-{pid}-63"))).unwrap();
    silico_core::file::save(&file, b"as read", b"formatted").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"formatted");
}

#[test]
fn a_save_over_a_file_swapped_for_a_pipe_is_refused_not_waited_on() {
    // A pipe put where the document was while it was rebuilt: with no reader, a write's
    // open that waits would wait forever; with one, it's no document. Found by the review.
    let dir = Scratch::new("save-fifo");
    let fifo = dir.0.join("part.silico");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let f = fifo.clone();
    std::thread::spawn(move || {
        let _ = tx.send(silico_core::file::save(&f, b"", b"x"));
    });
    let error = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("save waited on a pipe")
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoFailed);
    // With a reader: opened, and refused for what it is.
    let _reader = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)
        .unwrap();
    let error = silico_core::file::save(&fifo, b"", b"x").unwrap_err();
    assert!(
        error.to_string().contains("isn't a regular file now"),
        "{error}"
    );
}

#[test]
fn a_save_that_fails_says_its_paths_without_a_terminals_escapes() {
    // Found by the milestone's robustness review: the CLI quoted a file's name, but the
    // core's errors after it said paths as they were, so a directory named with an escape
    // sequence (a cloned repository's) reached the terminal through a failed save.
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("escaped");
    let dir = scratch.0.join("bad\u{1b}]0;owned\u{7}");
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let error = silico_core::file::save(&file, b"as read", b"formatted").unwrap_err();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(error.code(), ErrorCode::IoFailed);
    let words = error.to_string();
    assert!(!words.contains(['\u{1b}', '\u{7}']), "{words:?}");
    assert!(words.contains(r"bad\u{1b}]0;owned\u{7}"), "{words:?}");
}

/// Set to a file's path, for [`save_entry`]: the file to save over.
const SAVE_ENV: &str = "SILICO_FILE_TEST_SAVE";

/// Not a test of its own: run by [`a_save_cut_short_leaves_nothing_more_open_than_the_document`]
/// with [`SAVE_ENV`] set, this test binary saves 64 KiB over the file it names, which held
/// `as read`. Run normally, it does nothing.
#[test]
fn save_entry() {
    let Some(file) = std::env::var_os(SAVE_ENV) else {
        return;
    };
    let _ = silico_core::file::save(std::path::Path::new(&file), b"as read", &[b'x'; 1 << 16]);
    std::process::exit(0);
}

#[test]
fn a_save_cut_short_leaves_nothing_more_open_than_the_document() {
    // Found by the milestone's mutation run: the file a save writes beside the document
    // was made with every permission (mode | 0o777, not mode & 0o777) and narrowed only
    // before it was renamed, which passed every test. Killed by a file-size limit (a
    // shell's `ulimit -f`, SIGXFSZ) as it grows the new file, under no umask, the save
    // leaves that file. Cut short here while it grows, it's empty, so only its mode is
    // checked: a save cut short after writing would leave the next text of a document
    // others may not read, so others may not read the file either. On macOS the signal goes to the process, not the
    // thread that grew the file, which may run on and remove the file (or exit) before
    // the process ends: tried until a run leaves the file.
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::ExitStatusExt as _;
    let dir = Scratch::new("cut-short-mode");
    let file = dir.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let me = std::env::current_exe().unwrap();
    let mut left = None;
    for _ in 0..50 {
        // `exec`, so the shell's process is the save's: its number names the file beside.
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg(
                "umask 0; ulimit -f 1; exec \"$0\" --exact save_entry --nocapture --test-threads=1",
            )
            .arg(&me)
            .env(SAVE_ENV, &file)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let status = child.wait_with_output().unwrap().status;
        // Ended by the limit, or by the save's own exit after it failed: never saved.
        assert!(
            status.signal() == Some(libc::SIGXFSZ) || status.success(),
            "{status:?}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"as read");
        let beside = dir.0.join(format!(".part.silico.fmt-{pid}"));
        if let Ok(metadata) = std::fs::metadata(&beside) {
            left = Some(metadata.permissions().mode() & 0o777);
            break;
        }
    }
    let mode = left.expect("no save cut short left its file beside the document");
    assert_eq!(mode, 0o600, "{mode:o}");
}
