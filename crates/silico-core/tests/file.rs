//! M2.9: a document's file saved safely (`silico_core::file`), the cases a client meets
//! without timing: a file changed since it was read is left as it is, and the error, a
//! conflict, says to read it again.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/model.rs"]
mod model;

use model::WAIT;
use silico_core::ErrorCode;

/// `bytes` saved over `file`, which held `before` ([`silico_core::file::save_over`]): why
/// not, if not.
fn save(file: &std::path::Path, before: &[u8], bytes: &[u8]) -> Result<(), silico_core::Error> {
    silico_core::file::save_over(file, before, bytes).map_err(|unsaved| unsaved.error)
}

#[test]
fn a_save_over_a_file_changed_since_it_was_read_is_refused_and_says_to_read_it_again() {
    let dir = Scratch::new("save-changed");
    let file = dir.0.join("part.silico");
    std::fs::write(&file, b"edited").unwrap();
    let error = save(&file, b"as read", b"formatted").unwrap_err();
    // A conflict, not a failure of the disk (decision 16): reading it again is the fix.
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert!(error.to_string().contains("read it again"), "{error}");
    assert_eq!(std::fs::read(&file).unwrap(), b"edited");
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
    save(&file, b"as read", b"formatted").unwrap();
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
    save(&file, b"as read", b"formatted").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"formatted");
    assert_eq!(dir.names(), [left.clone(), "part.silico".to_owned()]);
    assert_eq!(std::fs::read(dir.0.join(&left)).unwrap(), b"left");
}

#[test]
fn a_document_whose_name_is_as_long_as_a_name_may_be_is_saved() {
    // Found by Worker 3: the file beside a document, `.NAME.fmt-PID`, passed the 255 bytes a
    // name may be when the document's own was near them, so its save failed. A name of 255
    // bytes, ASCII, and one of 254 in two-byte characters, are saved, by a rename and, with
    // another name for the file (a hard link), in place; nothing is left beside them.
    for (case, name) in [
        ("ascii", format!("{}.silico", "a".repeat(255 - 7))),
        ("utf8", "é".repeat(127)),
    ] {
        assert!(name.len() >= 254, "{case}");
        for linked in [false, true] {
            let dir = Scratch::new(&format!("long-{case}-{linked}"));
            let file = dir.0.join(&name);
            std::fs::write(&file, b"as read").unwrap();
            if linked {
                std::fs::hard_link(&file, dir.0.join("other")).unwrap();
            }
            save(&file, b"as read", b"formatted").unwrap();
            assert_eq!(
                std::fs::read(&file).unwrap(),
                b"formatted",
                "{case} {linked}"
            );
            let mut names = vec![name.clone()];
            if linked {
                assert_eq!(std::fs::read(dir.0.join("other")).unwrap(), b"formatted");
                names.insert(0, "other".to_owned());
            }
            names.sort();
            assert_eq!(dir.names(), names, "{case} {linked}");
        }
    }
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
    let error = save(&file, b"as read", b"formatted").unwrap_err();
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
    save(&file, b"as read", b"formatted").unwrap();
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
        let _ = tx.send(save(&f, b"", b"x"));
    });
    let error = rx
        .recv_timeout(WAIT)
        .expect("save waited on a pipe")
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoFailed);
    // With a reader: opened, and refused for what it is.
    let _reader = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)
        .unwrap();
    let error = save(&fifo, b"", b"x").unwrap_err();
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
    let error = save(&file, b"as read", b"formatted").unwrap_err();
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
    let _ = save(std::path::Path::new(&file), b"as read", &[b'x'; 1 << 16]);
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

/// The rules for a render's PNG in `workspace`, holding `held`.
fn png_rules<'a>(
    workspace: Option<&'a silico_core::file::Workspace>,
    held: &'a [std::path::PathBuf],
    replace: bool,
) -> silico_core::file::Output<'a> {
    silico_core::file::Output {
        field: "to",
        extensions: &["png"],
        writer: "a render",
        replace,
        workspace,
        held,
    }
}

/// `path` refused naming `to`, its message saying `says`, by the check before a write and by
/// the write alike; nothing written there.
fn refused_output(path: &std::path::Path, rules: &silico_core::file::Output, says: &str) {
    let before = std::fs::read(path).ok();
    let checked = silico_core::file::check_output(path, rules).unwrap_err();
    let e = silico_core::file::write_output(path, b"PNG", rules).unwrap_err();
    assert_eq!(
        (checked.message(), checked.parameter()),
        (e.message(), e.parameter())
    );
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
    assert_eq!(e.parameter(), Some("to"), "{e}");
    assert!(e.message().contains(says), "{e}");
    assert_eq!(std::fs::read(path).ok(), before, "{e}");
}

#[test]
fn a_png_spelled_otherwise_than_a_held_document_on_disk_is_that_document_and_refused() {
    // A held document named `Held.png` (the session's path, canonical, as it's on disk)
    // and a render to `HELD.png`, replacing what's there. On a case-insensitive file system
    // (macOS's default) that's the same file: refused, the document kept. On a
    // case-sensitive one (CI's Linux) it's another file, written beside it: there the fix
    // can't be told apart, so a gate on a Mac is what pins it.
    let dir = Scratch::new("held-case");
    let held = dir.0.join("Held.png");
    std::fs::write(&held, b"a document").unwrap();
    let held = [std::fs::canonicalize(&held).unwrap()];
    let other = dir.0.join("HELD.png");
    let rules = png_rules(None, &held, true);
    if other.exists() {
        refused_output(&other, &rules, "is a document a session holds");
        assert_eq!(std::fs::read(&held[0]).unwrap(), b"a document");
    } else {
        let wrote = silico_core::file::write_output(&other, b"PNG", &rules).unwrap();
        assert_eq!(wrote, std::fs::canonicalize(&other).unwrap());
        assert_eq!(std::fs::read(&held[0]).unwrap(), b"a document");
    }
}

#[test]
fn a_hard_link_to_a_held_document_is_that_document_and_refused() {
    // Another name of the held file (the same device and inode), on every file system: the
    // document, whatever the name's spelling, refused and kept, replace or not.
    let dir = Scratch::new("held-link");
    let held = dir.0.join("Held.png");
    std::fs::write(&held, b"a document").unwrap();
    let alias = dir.0.join("Alias.png");
    std::fs::hard_link(&held, &alias).unwrap();
    let holding = [held.clone()];
    for replace in [true, false] {
        refused_output(
            &alias,
            &png_rules(None, &holding, replace),
            "is a document a session holds",
        );
    }
    assert_eq!(std::fs::read(&held).unwrap(), b"a document");
    // A copy, another file, is no document: replaced.
    let copy = dir.0.join("Copy.png");
    std::fs::write(&copy, b"a document").unwrap();
    silico_core::file::write_output(&copy, b"PNG", &png_rules(None, &holding, true)).unwrap();
    assert_eq!(std::fs::read(&copy).unwrap(), b"PNG");
    assert_eq!(std::fs::read(&held).unwrap(), b"a document");
}

/// Set to a directory holding two empty directories, `a` and `b`, each a fresh tmpfs, for
/// [`devices_entry`].
#[cfg(target_os = "linux")]
const DEVICES_ENV: &str = "SILICO_FILE_TEST_DEVICES";

/// Not a test of its own: run by
/// [`a_file_on_another_device_with_a_held_documents_inode_is_another_file`] in a user and
/// mount namespace of its own, with [`DEVICES_ENV`] set. Run normally, it does nothing.
#[cfg(target_os = "linux")]
#[test]
fn devices_entry() {
    use std::os::unix::fs::MetadataExt;
    let Some(top) = std::env::var_os(DEVICES_ENV) else {
        return;
    };
    let top = std::path::PathBuf::from(top);
    let (a, b) = (top.join("a"), top.join("b"));
    // Each the first file made on its tmpfs, so each has the tmpfs's first file's inode:
    // the same number, on two devices.
    let held = a.join("part.silico");
    std::fs::write(&held, b"a document").unwrap();
    let target = b.join("view.png");
    std::fs::write(&target, b"theirs").unwrap();
    let (h, t) = (
        std::fs::metadata(&held).unwrap(),
        std::fs::metadata(&target).unwrap(),
    );
    assert_ne!(h.dev(), t.dev(), "two tmpfs mounts, one device");
    assert_eq!(
        h.ino(),
        t.ino(),
        "the first files of two fresh tmpfs mounts have different inodes"
    );
    // The held check (file.rs's `output_target`): another device, so another file, though
    // its inode is the held document's. Replaced, as any file there is with replace.
    let holding = [held.clone()];
    let checked = silico_core::file::check_output(&target, &png_rules(None, &holding, true));
    assert_eq!(
        checked.map_err(|e| e.to_string()),
        Ok(std::fs::canonicalize(&target).unwrap())
    );
    // The check is live here: the target's own hard link, held, is the target (its device
    // and inode both).
    let twin = b.join("twin.silico");
    std::fs::hard_link(&target, &twin).unwrap();
    refused_output(
        &target,
        &png_rules(None, &[twin], true),
        "is a document a session holds",
    );
    // `same_file` (the workspace's walk): the two mounts' roots are directories of one
    // inode on two devices. In a workspace of `a`, the walk reaches `top` from above and
    // goes on only through `a`, or another name of it: `b` isn't, so a path through it
    // leads out of the workspace, whatever is there. Were `b` taken for `a`, the walk would
    // go on in `a`, where `missing` isn't, and the path would be one that can't be reached.
    let (ra, rb) = (
        std::fs::metadata(&a).unwrap(),
        std::fs::metadata(&b).unwrap(),
    );
    assert_ne!(ra.dev(), rb.dev(), "two tmpfs mounts, one device");
    assert_eq!(
        ra.ino(),
        rb.ino(),
        "the roots of two fresh tmpfs mounts have different inodes"
    );
    let workspace = silico_core::file::Workspace::new(&a).unwrap();
    let rules = png_rules(Some(&workspace), &[], true);
    refused_output(
        &b.join("missing/view.png"),
        &rules,
        "is outside the workspace",
    );
    refused_output(
        &a.join("missing/view.png"),
        &rules,
        "its directory can't be reached",
    );
    // Said only past every check, so the run outside can tell it from the early return; on
    // stderr, since on stdout it would split libtest's own "test devices_entry ... ok".
    let _ = std::io::Write::write_all(&mut std::io::stderr(), b"devices checked\n");
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_on_another_device_with_a_held_documents_inode_is_another_file() {
    // Inode numbers repeat across file systems, so a held document is its device and its
    // inode both. Two fresh tmpfs mounts, in a user and mount namespace of the test's own
    // (`unshare -Urm`, as an unprivileged user may), give their first files the same inode
    // on two devices: the held document on one, an export's target on the other, which
    // isn't the document. Run in [`devices_entry`], this binary run again inside the
    // namespace, after the shell there mounts the two. A namespace that can't be made
    // fails the test, saying why.
    let dir = Scratch::new("devices");
    for name in ["a", "b"] {
        std::fs::create_dir(dir.0.join(name)).unwrap();
    }
    let me = std::env::current_exe().unwrap();
    let run = std::process::Command::new("unshare")
        .args(["-Urm", "sh", "-c"])
        .arg(
            "mount -t tmpfs none \"$1\" && mount -t tmpfs none \"$2\" && \
             exec \"$0\" --exact devices_entry --nocapture --test-threads=1",
        )
        .arg(&me)
        .arg(dir.0.join("a"))
        .arg(dir.0.join("b"))
        .env(DEVICES_ENV, &dir.0)
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
    // It ran, and passed: not filtered out by its name, nor returned early for want of
    // its variable.
    assert!(
        out.contains("test devices_entry ... ok") && err.contains("devices checked"),
        "stdout:\n{out}\nstderr:\n{err}"
    );
}

#[test]
fn a_png_is_written_whole_where_asked_and_its_canonical_path_given() {
    let dir = Scratch::new("output-new");
    let path = dir.0.join("view.png");
    // Checked first, it's where it will be written, and nothing is written yet.
    let checked = silico_core::file::check_output(&path, &png_rules(None, &[], false)).unwrap();
    assert!(!path.exists());
    let wrote =
        silico_core::file::write_output(&path, b"PNG bytes", &png_rules(None, &[], false)).unwrap();
    assert_eq!(checked, wrote);
    assert_eq!(wrote, std::fs::canonicalize(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"PNG bytes");
    // Nothing left beside it.
    let others: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(others, vec![std::ffi::OsString::from("view.png")]);
}

#[test]
fn a_file_there_is_refused_unless_replaced_and_replaced_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = Scratch::new("output-replace");
    let path = dir.0.join("view.png");
    std::fs::write(&path, b"old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    refused_output(
        &path,
        &png_rules(None, &[], false),
        &format!("\"{}\" exists", path.display()),
    );
    silico_core::file::write_output(&path, b"new", &png_rules(None, &[], true)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_document_its_cache_or_a_held_file_is_never_written() {
    let dir = Scratch::new("output-documents");
    // A .silico path (any case), whether or not it ends in .png otherwise.
    refused_output(
        &dir.0.join("part.silico"),
        &png_rules(None, &[], true),
        "is a document: a render never writes over one",
    );
    refused_output(
        &dir.0.join("part.SILICO"),
        &png_rules(None, &[], true),
        "is a document: a render never writes over one",
    );
    // In a .silico-cache directory.
    let cache = dir.0.join(".silico-cache");
    std::fs::create_dir(&cache).unwrap();
    refused_output(
        &cache.join("view.png"),
        &png_rules(None, &[], true),
        "lies in a .silico-cache directory",
    );
    // A document a session holds, reached through a link ending in .png.
    let held = dir.0.join("held.png");
    std::fs::write(&held, b"a document's bytes, named .png").unwrap();
    let link = dir.0.join("alias.png");
    std::os::unix::fs::symlink(&held, &link).unwrap();
    let holding = [held.clone()];
    refused_output(
        &link,
        &png_rules(None, &holding, true),
        "is a document a session holds",
    );
    refused_output(
        &held,
        &png_rules(None, &holding, true),
        "is a document a session holds",
    );
    // A link to a .silico file.
    let doc = dir.0.join("real.silico");
    std::fs::write(&doc, b"{}").unwrap();
    let to_doc = dir.0.join("sneaky.png");
    std::os::unix::fs::symlink(&doc, &to_doc).unwrap();
    refused_output(
        &to_doc,
        &png_rules(None, &[], true),
        "is a link to a document",
    );
    assert_eq!(std::fs::read(&doc).unwrap(), b"{}");
    // Each way of reaching a document said alike: the path, how it's a document, and that
    // the writer never writes over one.
    let said = |path: &std::path::Path, held: &[std::path::PathBuf]| {
        let e = silico_core::file::check_output(path, &png_rules(None, held, true)).unwrap_err();
        (e.message().to_owned(), e.suggestion().map(str::to_owned))
    };
    let shown = |path: &std::path::Path| format!("\"{}\"", path.display());
    let advice = Some("write it to a path that isn't a document's".to_owned());
    assert_eq!(
        said(&to_doc, &[]),
        (
            format!(
                "{} is a link to a document: a render never writes over one",
                shown(&to_doc)
            ),
            advice.clone()
        )
    );
    assert_eq!(
        said(&held, &holding),
        (
            format!(
                "{} is a document a session holds: a render never writes over one",
                shown(&held)
            ),
            advice
        )
    );
}

#[test]
fn a_path_without_the_extension_or_out_of_the_workspace_is_refused() {
    let dir = Scratch::new("output-workspace");
    let workspace = dir.0.join("work");
    let outside = dir.0.join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let root = silico_core::file::Workspace::new(&workspace).unwrap();
    let rules = png_rules(Some(&root), &[], true);
    refused_output(&workspace.join("view.jpg"), &rules, "doesn't end in .png");
    refused_output(&workspace.join("view"), &rules, "doesn't end in .png");
    refused_output(
        &outside.join("view.png"),
        &rules,
        "is outside the workspace",
    );
    refused_output(
        &workspace.join("../outside/view.png"),
        &rules,
        "is outside the workspace",
    );
    // Through a link out of it: a directory link, and a file link.
    std::os::unix::fs::symlink(&outside, workspace.join("out")).unwrap();
    refused_output(
        &workspace.join("out/view.png"),
        &rules,
        "is outside the workspace",
    );
    std::os::unix::fs::symlink(outside.join("target.png"), workspace.join("link.png")).unwrap();
    std::fs::write(outside.join("target.png"), b"theirs").unwrap();
    refused_output(
        &workspace.join("link.png"),
        &rules,
        "is outside the workspace",
    );
    assert_eq!(
        std::fs::read(outside.join("target.png")).unwrap(),
        b"theirs"
    );
    // A directory that doesn't exist, and a directory where the file would go.
    refused_output(
        &workspace.join("nowhere/view.png"),
        &rules,
        "its directory can't be reached",
    );
    std::fs::create_dir(workspace.join("dir.png")).unwrap();
    refused_output(&workspace.join("dir.png"), &rules, "isn't a file");
    // Inside, it's written.
    silico_core::file::write_output(&workspace.join("view.png"), b"PNG", &rules).unwrap();
}

#[test]
fn a_png_link_to_a_file_of_another_kind_is_refused_and_the_file_kept() {
    // A clone's `view.png` linked to its README: replacing it would write PNG bytes over the
    // README, kept as it was.
    let dir = Scratch::new("output-link-kind");
    let readme = dir.0.join("README.md");
    std::fs::write(&readme, b"keep me").unwrap();
    let view = dir.0.join("view.png");
    std::os::unix::fs::symlink(&readme, &view).unwrap();
    refused_output(
        &view,
        &png_rules(
            Some(&silico_core::file::Workspace::new(&dir.0).unwrap()),
            &[],
            true,
        ),
        "is a link to a file that isn't a .png",
    );
    assert_eq!(std::fs::read(&readme).unwrap(), b"keep me");
    // A link to a .png is written through, as before.
    let real = dir.0.join("real.png");
    std::fs::write(&real, b"old").unwrap();
    let alias = dir.0.join("alias.png");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    silico_core::file::write_output(
        &alias,
        b"new",
        &png_rules(
            Some(&silico_core::file::Workspace::new(&dir.0).unwrap()),
            &[],
            true,
        ),
    )
    .unwrap();
    assert_eq!(std::fs::read(&real).unwrap(), b"new");
}

#[test]
fn a_link_to_either_spelling_of_a_format_with_two_is_written_through_and_any_other_refused() {
    // STEP's two spellings: out.step linked to real.stp is a STEP file written through;
    // out2.stp linked to a README is refused naming both, the README kept.
    let dir = Scratch::new("output-link-two-spellings");
    let workspace = silico_core::file::Workspace::new(&dir.0).unwrap();
    let rules = silico_core::file::Output {
        field: "to",
        extensions: &["step", "stp"],
        writer: "an export",
        replace: true,
        workspace: Some(&workspace),
        held: &[],
    };
    let real = dir.0.join("real.stp");
    std::fs::write(&real, b"old").unwrap();
    let out = dir.0.join("out.step");
    std::os::unix::fs::symlink(&real, &out).unwrap();
    silico_core::file::write_output(&out, b"new", &rules).unwrap();
    assert_eq!(std::fs::read(&real).unwrap(), b"new");
    let readme = dir.0.join("README.md");
    std::fs::write(&readme, b"keep me").unwrap();
    let out2 = dir.0.join("out2.stp");
    std::os::unix::fs::symlink(&readme, &out2).unwrap();
    refused_output(
        &out2,
        &rules,
        "is a link to a file that isn't a .step or .stp",
    );
    assert_eq!(std::fs::read(&readme).unwrap(), b"keep me");
    // Not a link: a path of neither spelling names both.
    refused_output(
        &dir.0.join("out.stl"),
        &rules,
        "doesn't end in .step or .stp",
    );
}

/// The `n`th name a file a command writes is first written to, beside `name`, in this
/// process (`.NAME.part-PID`, then `-1`, `-2`, ...), by the rule's own words.
fn part(name: &str, n: u32) -> String {
    let pid = std::process::id();
    match n {
        0 => format!(".{name}.part-{pid}"),
        n => format!(".{name}.part-{pid}-{n}"),
    }
}

#[test]
fn a_png_takes_the_next_name_beside_it_and_with_all_64_taken_is_refused_writing_nothing() {
    // A run killed with this process number left `.view.png.part-PID`: the write goes on to
    // the next name, the left one as it was.
    let dir = Scratch::new("output-next-name");
    let path = dir.0.join("view.png");
    std::fs::write(dir.0.join(part("view.png", 0)), b"left").unwrap();
    silico_core::file::write_output(&path, b"PNG", &png_rules(None, &[], false)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"PNG");
    assert_eq!(
        std::fs::read(dir.0.join(part("view.png", 0))).unwrap(),
        b"left"
    );
    assert_eq!(dir.names(), [part("view.png", 0), "view.png".to_owned()]);
    // Every one of the 64 names taken: refused, saying so, nothing written.
    let dir = Scratch::new("output-names-taken");
    let path = dir.0.join("view.png");
    let mut taken: Vec<String> = (0..64).map(|n| part("view.png", n)).collect();
    for name in &taken {
        std::fs::write(dir.0.join(name), b"left").unwrap();
    }
    let e =
        silico_core::file::write_output(&path, b"PNG", &png_rules(None, &[], false)).unwrap_err();
    assert_eq!(e.code(), ErrorCode::IoFailed, "{e}");
    assert!(
        e.to_string()
            .contains("every name for a file beside it is taken"),
        "{e}"
    );
    taken.sort();
    assert_eq!(dir.names(), taken);
}

#[test]
fn a_png_in_a_directory_it_cant_write_in_says_why_at_once() {
    // Not a name taken, so not tried again under the next: the directory's own error.
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("output-unwritable");
    let shut = dir.0.join("shut");
    std::fs::create_dir(&shut).unwrap();
    std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o555)).unwrap();
    let e = silico_core::file::write_output(
        &shut.join("view.png"),
        b"PNG",
        &png_rules(None, &[], false),
    )
    .unwrap_err();
    std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(e.code(), ErrorCode::IoFailed, "{e}");
    assert!(e.to_string().contains("couldn't create a file in"), "{e}");
    assert_eq!(std::fs::read_dir(&shut).unwrap().count(), 0);
}

#[test]
fn a_png_named_up_to_255_bytes_with_its_part_suffix_is_written_beside_under_its_own_name() {
    // The name beside it, `.NAME.part-PID-NN`, is the name and its suffix and 8 bytes at
    // most (the dot, and `-63` with room to spare): kept where that's 255 bytes or fewer,
    // `silico-output` in its place past them. Seen by which names, all 64 taken, stop it.
    let suffix = format!(".part-{}", std::process::id()).len();
    let longest = 255 - 8 - suffix;
    let named = |len: usize| format!("{}.png", "a".repeat(len - 4));
    // At the most: its own name. Every `silico-output` name taken doesn't stop it.
    let dir = Scratch::new("output-long-name");
    for n in 0..64 {
        std::fs::write(dir.0.join(part("silico-output", n)), b"left").unwrap();
    }
    let at_most = dir.0.join(named(longest));
    silico_core::file::write_output(&at_most, b"PNG", &png_rules(None, &[], false)).unwrap();
    assert_eq!(std::fs::read(&at_most).unwrap(), b"PNG");
    // One byte more: `silico-output`. Every name of its own taken doesn't stop it.
    let dir = Scratch::new("output-too-long-name");
    let name = named(longest + 1);
    for n in 0..64 {
        std::fs::write(dir.0.join(part(&name, n)), b"left").unwrap();
    }
    let past = dir.0.join(&name);
    silico_core::file::write_output(&past, b"PNG", &png_rules(None, &[], false)).unwrap();
    assert_eq!(std::fs::read(&past).unwrap(), b"PNG");
    assert_eq!(dir.names().len(), 65);
}

#[test]
fn a_bare_file_name_is_checked_in_the_current_directory() {
    // A name with no directory, and no workspace: the process's own, as a shell takes it.
    // Checked only, so nothing is written.
    let name = format!("silico-bare-name-{}.png", std::process::id());
    let checked =
        silico_core::file::check_output(std::path::Path::new(&name), &png_rules(None, &[], false))
            .unwrap();
    let here = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    assert_eq!(checked, here.join(&name));
    assert!(!checked.exists());
}
