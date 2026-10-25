//! break-it for M2.9's `silico check`, `fmt`, and `show`: hostile paths and files (non-UTF-8
//! names, FIFOs, endless streams, symlinks, hard links, read-only files and directories,
//! names that look like options), a file edited while `fmt` rebuilds it, and kernel workers
//! that crash or are missing. Expected values come from the binary's own contract (its exit
//! codes: 0 clean, 1 the document has problems, 2 the command couldn't run; `--json`'s
//! line per file), POSIX file semantics, and what other formatters do (rustfmt, gofmt -w,
//! and prettier --write refuse a read-only file and write through a symlink).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{OsStr, OsString};
#[path = "common/run.rs"]
mod run;

use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

/// A directory of its own for one test, removed after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("adv-cli-{name}-{}", std::process::id()));
        let _ = make_writable(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: impl AsRef<OsStr>, text: &str) -> PathBuf {
        let file = self.0.join(name.as_ref());
        std::fs::write(&file, text).unwrap();
        file
    }
}

fn make_writable(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() && !path.is_symlink() {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = make_writable(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn worker() -> PathBuf {
    let worker = Path::new(env!("CARGO_BIN_EXE_silico")).with_file_name("silico-kernel-worker");
    assert!(
        worker.exists(),
        "{} isn't built: pixi run cargo build -p silico-kernel --bins",
        worker.display()
    );
    worker
}

/// `silico ARGS`, with `worker` as its kernel worker, the cache's secret in `scratch`.
fn command(scratch: &Scratch, worker: &Path, args: &[&OsStr]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_silico"));
    c.args(args)
        .current_dir(&scratch.0)
        .env("SILICO_KERNEL_WORKER", worker)
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run::ready(&mut c);
    c
}

struct Ran {
    status: Option<ExitStatus>,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }
}

/// Waits for `child` up to `deadline`, killing it after: `status` is `None` if it hung,
/// and then its stderr ends with where its time went ([`run::diagnosis`]).
fn finish(child: Child, deadline: Duration) -> Ran {
    let f = run::finish(child, deadline, true);
    let mut stderr = f.stderr.clone();
    if f.status.is_none() {
        stderr.push_str(&format!("\n(ran past {deadline:?}) {}", run::diagnosis(&f)));
    }
    Ran {
        status: f.status,
        stdout: String::from_utf8_lossy(&f.stdout).into_owned(),
        stderr,
    }
}

fn run(scratch: &Scratch, args: &[&OsStr]) -> Ran {
    run_with(scratch, &worker(), args)
}

fn run_with(scratch: &Scratch, worker: &Path, args: &[&OsStr]) -> Ran {
    let out = run::output(command(scratch, worker, args));
    Ran {
        status: Some(out.status),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn os(s: &str) -> &OsStr {
    OsStr::new(s)
}

/// A shell script standing in for the kernel worker.
fn script(scratch: &Scratch, name: &str, body: &str) -> PathBuf {
    let path = scratch.0.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo").arg(path).status().unwrap();
    assert!(status.success());
}

// --- Paths ----------------------------------------------------------------------------

#[test]
fn a_file_name_that_isnt_utf8_is_checked_not_a_panic() {
    // BREAKS: `std::env::args()` panics on an argument that isn't UTF-8 ("called
    // `Result::unwrap()` on an `Err` value: \"\\xFF.silico\""), exit 101, so a file Linux
    // or a script names in Latin-1 can't be checked at all. Expected: `args_os`, and the
    // file checked (exit 0); where the file system refuses such a name (APFS: EILSEQ), the
    // file can't exist, so exit 2, "couldn't be read".
    let scratch = Scratch::new("non-utf8");
    let name = OsString::from_vec(b"caf\xe9 \xff.silico".to_vec());
    let made = std::fs::write(scratch.0.join(&name), EXAMPLE).is_ok();
    let out = run(&scratch, &[os("check"), &name]);
    assert_ne!(out.code(), Some(101), "panicked: {}", out.stderr);
    assert_eq!(out.code(), Some(if made { 0 } else { 2 }), "{}", out.stderr);
}

#[test]
fn a_file_name_a_terminal_would_act_on_is_shown_escaped() {
    // Found by the milestone's robustness review: file names reached the terminal as they
    // were, so a cloned repository's `\e]0;...\a.silico` could retitle it, or hide lines.
    // Shown quoted, those characters escaped; a JSON line's name is the name, which JSON
    // escapes.
    let scratch = Scratch::new("escapes");
    let name = "bad\u{1b}]0;owned\u{7}\u{200b}.silico";
    scratch.file(name, "{");
    let out = run(&scratch, &[os("check"), os(name)]);
    assert_eq!(out.code(), Some(1), "{}", out.stderr);
    assert!(
        !out.stderr.contains(['\u{1b}', '\u{7}', '\u{200b}']),
        "{:?}",
        out.stderr
    );
    assert!(
        out.stderr
            .starts_with(r#""bad\u{1b}]0;owned\u{7}\u{200b}.silico": "#),
        "{:?}",
        out.stderr
    );
    let out = run(&scratch, &[os("check"), os("--json"), os(name)]);
    let line: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    assert_eq!(line["file"], name);
    assert!(!out.stdout.contains('\u{1b}'), "{:?}", out.stdout);
    // A plain name is shown as it is.
    scratch.file("plain.silico", "{");
    let out = run(&scratch, &[os("check"), os("plain.silico")]);
    assert!(out.stderr.starts_with("plain.silico: "), "{:?}", out.stderr);
}

#[test]
fn double_dash_ends_the_options_so_a_file_may_be_named_like_one() {
    // BREAKS: `--` is taken as an unknown option ("silico check: no option \"--\""), and
    // anything starting `--` is an option wherever it is, so a file named `--json` can be
    // checked only as `./--json`. Expected: POSIX's `--` (utility syntax guideline 10).
    let scratch = Scratch::new("dashes");
    scratch.file("--json", EXAMPLE);
    let out = run(&scratch, &[os("check"), os("--"), os("--json")]);
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
}

#[test]
fn paths_with_spaces_single_dashes_and_unicode_are_files() {
    let scratch = Scratch::new("odd-names");
    for name in ["a part.silico", "-x.silico", "-", "部品 ✓.silico", " lead"] {
        scratch.file(name, EXAMPLE);
        let out = run(&scratch, &[os("check"), os(name)]);
        assert_eq!(out.code(), Some(0), "{name:?}: {}", out.stderr);
        let out = run(&scratch, &[os("show"), os(name)]);
        assert_eq!(out.code(), Some(0), "{name:?}: {}", out.stderr);
        assert!(
            out.stdout.starts_with("k7f2qx3m \"Block\" box"),
            "{}",
            out.stdout
        );
    }
}

#[test]
fn a_directory_is_exit_2_for_every_command() {
    let scratch = Scratch::new("dir");
    std::fs::create_dir(scratch.0.join("d.silico")).unwrap();
    for c in ["check", "fmt", "show"] {
        let out = run(&scratch, &[os(c), os("d.silico")]);
        assert_eq!(out.code(), Some(2), "{c}: {}", out.stderr);
        assert!(out.stderr.contains("couldn't be read"), "{}", out.stderr);
    }
}

// --- Files that aren't files ----------------------------------------------------------

#[test]
fn a_fifo_with_no_writer_is_refused_not_waited_on() {
    // BREAKS: `std::fs::read` opens the FIFO and blocks in open(2) until a writer comes,
    // forever: `silico show part.silico` (or an agent's `check`) hangs. Expected: a
    // document is a regular file; anything else is exit 2 at once, "isn't a regular file".
    let scratch = Scratch::new("fifo");
    mkfifo(&scratch.0.join("part.silico"));
    for c in ["show", "check", "fmt"] {
        let child = command(&scratch, &worker(), &[os(c), os("part.silico")])
            .spawn()
            .unwrap();
        let out = finish(child, Duration::from_secs(10));
        assert!(out.status.is_some(), "silico {c} on a FIFO hung for 10s");
        assert_eq!(out.code(), Some(2), "{c}: {}", out.stderr);
    }
}

#[test]
fn reading_stops_at_the_most_a_document_may_be() {
    // Found by /break-it: the reader refuses more than MAX_INPUT_LEN (64 MiB) bytes, but
    // the CLI read the whole file first with `std::fs::read`, which has no limit: `silico
    // show /dev/zero` reached 3.2 GB of memory in a second, and a stream of any size was
    // read to its end. Here a FIFO is fed 256 MiB: it isn't a regular file, so it's refused
    // (exit 2) before anything past a pipe's buffer is read; a regular file is read to one
    // byte past the limit (the next test).
    let scratch = Scratch::new("endless");
    let fifo = scratch.0.join("part.silico");
    mkfifo(&fifo);
    // Both ends opened here, before silico runs: a read end of the test's own, held until
    // silico has finished, and the write end the writer feeds. An open in the writer's
    // thread raced silico (which opens the FIFO, refuses it, and closes it): opened after,
    // with no reader left, it waited forever. Dropping the read end ends the writer: the
    // write it's waiting in, or the next, fails (EPIPE).
    let held = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .unwrap();
    // With a reader there already, the write end's open returns at once, and its writes
    // wait for room, so silico has a stream to read.
    let mut f = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    let child = command(&scratch, &worker(), &[os("show"), os("part.silico")])
        .spawn()
        .unwrap();
    let writer = std::thread::spawn(move || {
        let chunk = vec![b' '; 1 << 20];
        let mut written = 0usize;
        while written < 256 << 20 {
            match f.write(&chunk) {
                Ok(n) => written += n,
                Err(_) => break,
            }
        }
        written
    });
    let out = finish(child, Duration::from_secs(60));
    drop(held);
    let written = writer.join().unwrap();
    assert!(out.status.is_some(), "hung");
    // Nothing read before it's refused: what was written is at most a chunk and a pipe's
    // buffer, which the test's own read end (never read) holds. (Found by the review: a
    // bound of the 64 MiB a document may be passed a silico that read that much first.)
    assert!(
        written <= 2 << 20,
        "silico read {written} bytes of the FIFO before refusing it"
    );
    assert_eq!(out.code(), Some(2), "{}", out.stderr);
    assert!(
        out.stderr.contains("isn't a regular file"),
        "{}",
        out.stderr
    );
}

#[test]
fn a_regular_file_past_the_limit_is_refused_saying_the_limit() {
    let scratch = Scratch::new("big");
    let limit = silico_core::document::json::MAX_INPUT_LEN;
    let f = std::fs::File::create(scratch.0.join("part.silico")).unwrap();
    f.set_len(limit as u64 + 1).unwrap();
    let out = run(&scratch, &[os("show"), os("part.silico")]);
    assert_eq!(out.code(), Some(1), "{}", out.stderr);
    assert!(out.stderr.contains(&limit.to_string()), "{}", out.stderr);
}

// --- fmt's whole-file write -----------------------------------------------------------

#[test]
fn fmt_through_a_symlink_formats_the_file_it_names_and_keeps_the_link() {
    // BREAKS: `write_whole` renames its new file over the path given, so `silico fmt
    // link.silico` replaces the symlink with a regular file holding the formatted text and
    // leaves the real document as it was, exit 0: the link (into a shared parts library, a
    // git-annex or Nix store, a dotfiles repo) is gone, and the document it named was never
    // formatted. Expected: the target formatted, the link kept (rustfmt, gofmt -w, prettier
    // all write through it), or exit 2 refusing a link.
    let scratch = Scratch::new("symlink");
    std::fs::create_dir(scratch.0.join("library")).unwrap();
    let real = scratch.file("library/real.silico", EXAMPLE);
    std::os::unix::fs::symlink("library/real.silico", scratch.0.join("link.silico")).unwrap();
    let out = run(&scratch, &[os("fmt"), os("link.silico")]);
    let link = std::fs::symlink_metadata(scratch.0.join("link.silico")).unwrap();
    let real_text = std::fs::read_to_string(&real).unwrap();
    // `fmt` writes through a link (docs/cli.md), so it's done, not refused.
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
    assert!(
        link.file_type().is_symlink(),
        "the link was replaced by a file"
    );
    assert!(
        real_text.contains(r#""fingerprint": {"volume": 8000,"#),
        "the real document wasn't formatted: {real_text}"
    );
}

#[test]
fn fmt_of_a_hard_linked_document_changes_every_name_of_it() {
    // BREAKS: rename-over gives `a.silico` a new inode, so its hard link `b.silico` keeps
    // the old text: one document becomes two that disagree, silently. Expected: both names
    // read the formatted text (a write in place), or exit 2 saying it has other links.
    let scratch = Scratch::new("hardlink");
    let a = scratch.file("a.silico", EXAMPLE);
    let b = scratch.0.join("b.silico");
    std::fs::hard_link(&a, &b).unwrap();
    let out = run(&scratch, &[os("fmt"), os("a.silico")]);
    // Written in place, so every name reads the formatted text: done, not refused.
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
    assert_eq!(
        std::fs::metadata(&a).unwrap().ino(),
        std::fs::metadata(&b).unwrap().ino(),
        "the hard link was split"
    );
    for name in [&a, &b] {
        let text = std::fs::read_to_string(name).unwrap();
        assert!(
            text.contains(r#""fingerprint": {"volume": 8000,"#),
            "{}: {text}",
            name.display()
        );
    }
}

#[test]
fn fmt_doesnt_rewrite_a_read_only_file() {
    // BREAKS: a file marked 0444 (a vendored or released part, kept read-only on purpose)
    // is rewritten anyway, since rename needs only the directory to be writable, and its
    // 0444 is copied onto the new file, so it even looks untouched. Expected: exit 2, the
    // file as it was ("couldn't be written: Permission denied"), as rustfmt, gofmt -w, and
    // prettier --write do.
    let scratch = Scratch::new("readonly");
    let file = scratch.file("part.silico", EXAMPLE);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let out = run(&scratch, &[os("fmt"), os("part.silico")]);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        EXAMPLE,
        "a read-only file was rewritten"
    );
    assert_eq!(out.code(), Some(2), "{}", out.stderr);
}

#[test]
fn fmt_doesnt_rewrite_a_file_its_owner_cant_write() {
    // Found by the review: 0464 (the owner's write bit off, the group's on, common where
    // each user has a group of their own) isn't `readonly()`, which looks for no write bit
    // at all, so `fmt` renamed a new file over one its user can't write. What the system
    // says this process may do decides.
    let scratch = Scratch::new("owner-cant-write");
    let file = scratch.file("part.silico", EXAMPLE);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o464)).unwrap();
    let out = run(&scratch, &[os("fmt"), os("part.silico")]);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), EXAMPLE);
    assert_eq!(out.code(), Some(2), "{}", out.stderr);
    assert!(out.stderr.contains("io_failed"), "{}", out.stderr);
    assert!(out.stderr.contains("Permission denied"), "{}", out.stderr);
}

#[test]
fn fmt_of_a_hard_linked_document_cut_short_loses_neither_name_of_it() {
    // Found by the review: written in place, the document was truncated first, so a write
    // cut short (here, a file-size limit of 0, which kills at the first byte written)
    // left it empty under both names. The cache is warmed first, so the document is the
    // first file written.
    let scratch = Scratch::new("hardlink-cut-short");
    let a = scratch.file("a.silico", EXAMPLE);
    let b = scratch.0.join("b.silico");
    std::fs::hard_link(&a, &b).unwrap();
    let warm = run(&scratch, &[os("fmt"), os("--check"), os("a.silico")]);
    assert_eq!(warm.code(), Some(1), "{}", warm.stderr);
    // `silico fmt a.silico` as `command` runs it, through a shell that sets the limit.
    let silico = command(&scratch, &worker(), &[]);
    let mut limited = Command::new("sh");
    limited
        .arg("-c")
        .arg("ulimit -f 0; exec \"$0\" fmt a.silico")
        .arg(silico.get_program())
        .current_dir(&scratch.0)
        .stdin(Stdio::null());
    for (key, value) in silico.get_envs() {
        match value {
            Some(value) => limited.env(key, value),
            None => limited.env_remove(key),
        };
    }
    // `exec`, so the shell's process is silico's: its number names the file it keeps.
    let child = limited
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let limited = child.wait_with_output().unwrap();
    assert!(!limited.status.success());
    for name in [&a, &b] {
        assert_eq!(
            std::fs::read_to_string(name).unwrap(),
            EXAMPLE,
            "{}",
            name.display()
        );
    }
    assert_eq!(
        std::fs::metadata(&a).unwrap().ino(),
        std::fs::metadata(&b).unwrap().ino()
    );
    // Killed while keeping the old text: the file it kept it in is left, by the name
    // docs/cli.md gives (here empty: the limit killed it at its first byte).
    let kept = format!(".a.silico.fmt-{pid}");
    assert!(ls(&scratch.0).contains(&kept), "{:?}", ls(&scratch.0));
}

#[test]
fn fmt_in_a_read_only_directory_is_exit_2_and_doesnt_claim_it_wrote() {
    // BREAKS (low): the file is left whole, exit 2, but stderr first says "wrote the
    // fingerprint of k7f2qx3m", then "couldn't be written": a reader (or an agent reading
    // stderr line by line) is told a fingerprint was written that never was.
    let scratch = Scratch::new("rodir");
    std::fs::create_dir(scratch.0.join("ro")).unwrap();
    let file = scratch.file("ro/part.silico", EXAMPLE);
    std::fs::set_permissions(scratch.0.join("ro"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let out = run(&scratch, &[os("fmt"), os("ro/part.silico")]);
    assert_eq!(out.code(), Some(2), "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), EXAMPLE);
    let entries: Vec<_> = std::fs::read_dir(scratch.0.join("ro")).unwrap().collect();
    assert_eq!(entries.len(), 1, "nothing left beside it");
    assert!(out.stderr.contains("couldn't be written"), "{}", out.stderr);
    // Why, as the system says: the directory, not a taken name.
    assert!(
        out.stderr.contains("couldn't create a file in")
            && out.stderr.contains("Permission denied"),
        "{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("wrote the fingerprint"),
        "{}",
        out.stderr
    );
}

#[test]
fn fmt_doesnt_overwrite_an_edit_made_while_it_rebuilt() {
    // BREAKS: fmt reads the file, rebuilds it (seconds to minutes for a real part), then
    // renames its text over whatever the file holds by then: an edit saved meanwhile (the
    // user's editor, an agent, `git checkout`) is lost without a word, exit 0. Here the
    // edit (the block 41 long) lands while fmt rebuilds, every time: the worker says it's
    // starting (fmt reads the file before it starts a worker), then waits for the edit
    // before it serves. (Once a 1 s sleep against a 3 s start, which a loaded machine
    // could turn into an edit before fmt read the file, so the test could pass without the
    // race.) Expected: the edit kept, byte for byte, and fmt refusing, saying why (exit 2).
    let scratch = Scratch::new("race");
    let file = scratch.file("part.silico", EXAMPLE);
    let started = scratch.0.join("started");
    let edited_mark = scratch.0.join("edited");
    let slow = script(
        &scratch,
        "slow-worker",
        &format!(
            "touch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\nexec '{}' \"$@\"",
            started.display(),
            edited_mark.display(),
            worker().display()
        ),
    );
    let mut child = command(&scratch, &slow, &[os("fmt"), os("part.silico")])
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + run::DEADLINE;
    while !started.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "fmt ended before it started a worker"
        );
        assert!(std::time::Instant::now() < deadline, "no worker started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let edited = EXAMPLE.replace(r#""length": 40"#, r#""length": 41"#);
    std::fs::write(&file, &edited).unwrap();
    std::fs::write(&edited_mark, "").unwrap();
    let out = finish(child, run::DEADLINE);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        edited,
        "the edit was lost (exit {:?}): {}",
        out.code(),
        out.stderr
    );
    assert_eq!(out.code(), Some(2), "{}", out.stderr);
    assert!(
        out.stderr.contains("changed while it was rebuilt"),
        "{}",
        out.stderr
    );
}

#[test]
fn two_fmts_at_once_leave_one_canonical_file_and_nothing_beside_it() {
    let scratch = Scratch::new("two-fmts");
    let file = scratch.file("part.silico", EXAMPLE);
    let a = command(&scratch, &worker(), &[os("fmt"), os("part.silico")])
        .spawn()
        .unwrap();
    let b = command(&scratch, &worker(), &[os("fmt"), os("part.silico")])
        .spawn()
        .unwrap();
    let (a, b) = (finish(a, run::DEADLINE), finish(b, run::DEADLINE));
    // Each writes, or finds the file the other wrote changed under it and leaves it to
    // them (exit 2, saying so), never writing over it: at least one writes.
    let ok_or_left = |out: &Ran| {
        out.code() == Some(0)
            || (out.code() == Some(2) && out.stderr.contains("changed while it was rebuilt"))
    };
    assert!(ok_or_left(&a), "{}", a.stderr);
    assert!(ok_or_left(&b), "{}", b.stderr);
    assert!(a.code() == Some(0) || b.code() == Some(0));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains(r#""fingerprint": {"volume": 8000,"#),
        "{text}"
    );
    let out = run(&scratch, &[os("fmt"), os("--check"), os("part.silico")]);
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
    let mut left: Vec<String> = std::fs::read_dir(&scratch.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, [".silico-cache", "cache-home", "part.silico"]);
}

#[test]
fn fmt_of_a_document_its_fingerprints_would_take_past_the_limit_leaves_it_and_says_why() {
    // Notes take the document to within 64 bytes of the 16 MiB limit: it reads, but a
    // fingerprint won't fit. fmt says so (exit 1) and the file is as it was.
    let scratch = Scratch::new("at-limit");
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN;
    let canonical =
        silico_core::document::write(&silico_core::document::read(EXAMPLE.as_bytes()).unwrap())
            .unwrap();
    let pad = limit - canonical.len() - 64 - r#", "note": """#.len();
    let text = canonical.replacen(
        r#""height": 10}"#,
        &format!(r#""height": 10, "note": "{}"}}"#, "x".repeat(pad)),
        1,
    );
    assert!(text.len() <= limit);
    let file = scratch.file("part.silico", &text);
    let out = run(&scratch, &[os("check"), os("part.silico")]);
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
    let out = run(&scratch, &[os("fmt"), os("part.silico")]);
    assert_eq!(out.code(), Some(1), "{}", out.stderr);
    assert!(out.stderr.contains(&limit.to_string()), "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}

// --- Kernel workers that crash or are missing -----------------------------------------

#[test]
fn a_kernel_that_crashes_is_exit_2_not_a_problem_with_the_document() {
    // BREAKS: a worker that exits as soon as it starts makes every feature fail (now with
    // worker_unavailable: it ended before its first frame), and `check` calls that a problem with the document: exit 1 and
    // `{"ok": false}`, so an agent "fixes" a sound document; `fmt` prints the crash and
    // exits 0, after rewriting the file without the fingerprints it promised. The binary's
    // contract: 1 is "a document has problems", 2 is "a kernel that couldn't start".
    let scratch = Scratch::new("crash");
    let file = scratch.file("part.silico", EXAMPLE);
    let dies = script(&scratch, "dies", "exit 3");
    let out = run_with(&scratch, &dies, &[os("check"), os("part.silico")]);
    assert!(out.stderr.contains("worker_unavailable"), "{}", out.stderr);
    assert_eq!(out.code(), Some(2), "check: {}", out.stderr);
    let out = run_with(&scratch, &dies, &[os("fmt"), os("part.silico")]);
    assert_eq!(out.code(), Some(2), "fmt: {}", out.stderr);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), EXAMPLE);
}

#[test]
fn a_document_that_doesnt_read_is_its_problem_even_with_no_kernel() {
    // BREAKS: `check` and `fmt` start the kernel before they read the document, so with no
    // worker installed (or one that won't start) a file that isn't JSON at all is exit 2,
    // "SILICO_KERNEL_WORKER names ... which isn't a file", and never its own problem;
    // `show` reports it (exit 1) without a kernel. Expected: the read and the gate first,
    // exit 1 naming the bad text.
    let scratch = Scratch::new("no-kernel");
    scratch.file("part.silico", "{");
    for c in ["check", "fmt"] {
        let out = run_with(
            &scratch,
            Path::new("/nonexistent"),
            &[os(c), os("part.silico")],
        );
        assert_eq!(out.code(), Some(1), "{c}: {}", out.stderr);
        assert!(
            out.stderr.contains("isn't valid JSON"),
            "{c}: {}",
            out.stderr
        );
    }
}

#[test]
fn a_missing_kernel_is_exit_2_saying_what_to_fix() {
    let scratch = Scratch::new("missing-kernel");
    scratch.file("part.silico", EXAMPLE);
    let out = run_with(
        &scratch,
        Path::new("/nonexistent"),
        &[os("check"), os("part.silico")],
    );
    assert_eq!(out.code(), Some(2), "{}", out.stderr);
    assert!(
        out.stderr.contains("SILICO_KERNEL_WORKER"),
        "{}",
        out.stderr
    );
}

/// One sketch, a line, and nothing built from it: what it asks of Silico is the sketch
/// worker's solve, not the kernel's.
const SKETCH: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}
  ]
}
"#;

#[test]
fn a_worker_that_cant_be_had_is_worker_unavailable_exit_2_and_the_file_left_as_it_is() {
    // The M3 review's start-code decision: a worker named as a directory, a path with
    // nothing at it, or a file that can't be run can't be had (worker_unavailable), for the
    // kernel and the sketch worker alike. It's Silico's install, not the document: check,
    // check --json, and fmt exit 2, saying so, and the file is left as it is.
    let scratch = Scratch::new("unavailable");
    let directory = scratch.0.join("a-directory");
    std::fs::create_dir(&directory).unwrap();
    let missing = scratch.0.join("missing");
    // Written as any file is: readable, not executable.
    let not_runnable = scratch.file("not-runnable", "#!/bin/sh\nexit 0\n");
    for (variable, text) in [
        ("SILICO_KERNEL_WORKER", EXAMPLE),
        ("SILICO_SKETCH_WORKER", SKETCH),
    ] {
        for (what, path) in [
            ("a directory", &directory),
            ("missing", &missing),
            ("not runnable", &not_runnable),
        ] {
            let file = scratch.file("part.silico", text);
            for args in [&["check"][..], &["check", "--json"], &["fmt"]] {
                let mut c = command(
                    &scratch,
                    &worker(),
                    &args
                        .iter()
                        .map(|a| os(a))
                        .chain([os("part.silico")])
                        .collect::<Vec<_>>(),
                );
                c.env(variable, path);
                let out = run::output(c);
                let (stdout, stderr) = (
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr),
                );
                let case = format!("{variable} {what}, {args:?}");
                assert_eq!(out.status.code(), Some(2), "{case}: {stderr}");
                if args.contains(&"--json") {
                    let line: serde_json::Value = serde_json::from_str(stdout.trim())
                        .unwrap_or_else(|e| panic!("{case}: {e}: {stdout}"));
                    assert_eq!(line["ok"], false, "{case}: {line}");
                    assert_eq!(
                        line["error"]["code"], "worker_unavailable",
                        "{case}: {line}"
                    );
                } else {
                    assert!(stderr.contains("worker_unavailable"), "{case}: {stderr}");
                }
                assert_eq!(std::fs::read_to_string(&file).unwrap(), text, "{case}");
            }
        }
    }
}

// --- What it says ---------------------------------------------------------------------

#[test]
fn check_json_says_a_line_for_every_file_even_one_it_couldnt_read() {
    // BREAKS: `check --json missing.silico part.silico` prints one JSON line (the part's)
    // and the missing file's error only as text on stderr: an agent reading stdout sees
    // one result for two files, and can't tell which wasn't checked or why without
    // scraping stderr. Expected: a line for each file, in order, the unreadable one
    // `"ok": false` with its error (as the `problems` contract has it).
    let scratch = Scratch::new("json-missing");
    scratch.file("part.silico", EXAMPLE);
    let out = run(
        &scratch,
        &[
            os("check"),
            os("--json"),
            os("missing.silico"),
            os("part.silico"),
        ],
    );
    assert_eq!(out.code(), Some(2), "{}", out.stderr);
    let lines: Vec<serde_json::Value> = out
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "stdout: {}", out.stdout);
    assert_eq!(lines[0]["file"], "missing.silico");
    assert_eq!(lines[0]["ok"], false);
    // The error in the core's contract shape (tests/contract/check-line.json).
    assert_eq!(lines[0]["error"]["code"], "io_failed", "{}", lines[0]);
    assert!(
        lines[0]["error"]["message"]
            .as_str()
            .is_some_and(|e| e.contains("couldn't be read")),
        "{}",
        lines[0]
    );
    assert_eq!(lines[1]["ok"], true, "{}", lines[1]);
}

#[test]
fn check_of_several_files_reports_each_and_exits_with_the_worst() {
    let scratch = Scratch::new("several");
    scratch.file("good.silico", EXAMPLE);
    scratch.file(
        "bad.silico",
        &EXAMPLE.replace(r#""length": 40"#, r#""length": 0"#),
    );
    let out = run(
        &scratch,
        &[
            os("check"),
            os("--json"),
            os("good.silico"),
            os("bad.silico"),
        ],
    );
    assert_eq!(out.code(), Some(1), "{}", out.stderr);
    let lines: Vec<serde_json::Value> = out
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[1]["ok"], false);
    assert_eq!(lines[1]["problems"][0]["parameter"], "length");
}

#[test]
fn more_problems_than_listed_say_how_many_more() {
    let scratch = Scratch::new("many");
    let features: Vec<String> = (0..150)
        .map(|i| format!(r#"{{"id": "b{i:07}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 0, "width": 1, "height": 1}}"#))
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
        features.join(",")
    );
    scratch.file("part.silico", &text);
    let out = run(&scratch, &[os("check"), os("part.silico")]);
    assert_eq!(out.code(), Some(1));
    assert!(
        out.stderr.contains("part.silico: and 50 more problems"),
        "{}",
        out.stderr
    );
    let out = run(&scratch, &[os("check"), os("--json"), os("part.silico")]);
    let line: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    assert_eq!(line["total"], 150);
    assert_eq!(line["problems"].as_array().unwrap().len(), 100);
}

#[test]
fn an_error_on_stderr_says_its_code_once() {
    // BREAKS (low): `report` writes the code, then the error's `Display`, which starts with
    // the code too: "part.silico: invalid_input: invalid_input: the document isn't valid
    // JSON". Expected: the code once.
    let scratch = Scratch::new("code-once");
    scratch.file("part.silico", "{");
    let out = run(&scratch, &[os("show"), os("part.silico")]);
    assert_eq!(out.code(), Some(1));
    assert!(
        !out.stderr.contains("invalid_input: invalid_input"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.starts_with("part.silico: invalid_input: "),
        "{}",
        out.stderr
    );
}

#[test]
fn show_into_a_closed_pipe_fails_quietly_not_a_panic() {
    // A 10 MB name, so the outline is far more than a pipe holds, and the reader gone.
    let scratch = Scratch::new("epipe");
    scratch.file(
        "part.silico",
        &EXAMPLE.replace(r#""Block""#, &format!("\"{}\"", "n".repeat(10 << 20))),
    );
    let mut child = command(&scratch, &worker(), &[os("show"), os("part.silico")])
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let status = child.wait().unwrap();
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert_eq!(status.code(), Some(2), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn show_of_bytes_that_arent_utf8_is_exit_1_saying_so() {
    let scratch = Scratch::new("binary");
    let path = scratch.0.join("part.silico");
    let mut bytes = EXAMPLE.as_bytes().to_vec();
    bytes.splice(60..60, [0xff, 0xfe, 0x00]);
    std::fs::write(&path, bytes).unwrap();
    let out = run(&scratch, &[os("show"), os("part.silico")]);
    assert_eq!(out.code(), Some(1), "{}", out.stderr);
    assert!(out.stderr.contains("UTF-8"), "{}", out.stderr);
    assert!(out.stdout.is_empty());
}

// --- Found by the review's second round ------------------------------------------------

/// The names in `dir`, sorted.
fn ls(dir: &Path) -> Vec<String> {
    let mut left: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    left
}

#[test]
fn fmt_of_a_hard_linked_document_whose_write_fails_puts_its_text_back() {
    // The in-place write's failure path: the write fails (EFBIG, under a file-size limit
    // with SIGXFSZ ignored) after the old text was kept beside it, and the old text goes
    // back, under both names, with nothing left beside them.
    let scratch = Scratch::new("hardlink-write-fails");
    // Padded to 1000 bytes: under a 1024-byte file-size limit the old text fits, the new
    // (with a fingerprint) doesn't.
    let pad = 1000 - EXAMPLE.len();
    let before = EXAMPLE.replace(
        r#""name": "Block""#,
        &format!(r#""name": "Block{}""#, "x".repeat(pad)),
    );
    assert_eq!(before.len(), 1000);
    let a = scratch.file("a.silico", &before);
    let b = scratch.0.join("b.silico");
    std::fs::hard_link(&a, &b).unwrap();
    let warm = run(&scratch, &[os("fmt"), os("--check"), os("a.silico")]);
    assert_eq!(warm.code(), Some(1), "{}", warm.stderr);
    // ulimit -f's block (512 or 1024 bytes, by shell), measured.
    let probe = scratch.0.join("probe");
    let _ = Command::new("sh")
        .arg("-c")
        .arg("trap '' XFSZ; ulimit -f 1; head -c 4096 /dev/zero > \"$0\"")
        .arg(&probe)
        .status();
    let block = std::fs::metadata(&probe).unwrap().len();
    std::fs::remove_file(&probe).unwrap();
    let silico = command(&scratch, &worker(), &[]);
    let mut limited = Command::new("sh");
    limited
        .arg("-c")
        .arg(format!(
            "trap '' XFSZ; ulimit -f {}; exec \"$0\" fmt a.silico",
            1024 / block
        ))
        .arg(silico.get_program())
        .current_dir(&scratch.0)
        .stdin(Stdio::null());
    for (key, value) in silico.get_envs() {
        match value {
            Some(value) => limited.env(key, value),
            None => limited.env_remove(key),
        };
    }
    let out = limited.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("it's as it was"), "{stderr}");
    // The limit's advice, not permissions' or a full disk's.
    assert!(stderr.contains("ulimit -f"), "{stderr}");
    for name in [&a, &b] {
        assert_eq!(
            std::fs::read_to_string(name).unwrap(),
            before,
            "{}",
            name.display()
        );
    }
    assert_eq!(
        ls(&scratch.0),
        [".silico-cache", "a.silico", "b.silico", "cache-home"]
    );
}

#[test]
fn files_in_two_directories_each_have_their_own_cache() {
    // One pool, but an engine for each directory, its cache beside its files.
    let scratch = Scratch::new("two-dirs");
    std::fs::create_dir(scratch.0.join("one")).unwrap();
    std::fs::create_dir(scratch.0.join("two")).unwrap();
    scratch.file("one/a.silico", EXAMPLE);
    scratch.file("two/b.silico", EXAMPLE);
    let out = run(
        &scratch,
        &[os("check"), os("one/a.silico"), os("two/b.silico")],
    );
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
    assert_eq!(ls(&scratch.0.join("one")), [".silico-cache", "a.silico"]);
    assert_eq!(ls(&scratch.0.join("two")), [".silico-cache", "b.silico"]);
}

#[test]
fn the_cache_of_a_linked_document_is_beside_the_file_it_names() {
    let scratch = Scratch::new("link-cache");
    std::fs::create_dir(scratch.0.join("library")).unwrap();
    scratch.file("library/real.silico", EXAMPLE);
    std::os::unix::fs::symlink("library/real.silico", scratch.0.join("link.silico")).unwrap();
    let out = run(&scratch, &[os("check"), os("link.silico")]);
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
    assert_eq!(
        ls(&scratch.0.join("library")),
        [".silico-cache", "real.silico"]
    );
    assert_eq!(ls(&scratch.0), ["cache-home", "library", "link.silico"]);
}

#[test]
fn fmt_of_a_hard_linked_document_killed_by_a_size_limit_mid_write_leaves_it_as_it_was() {
    // Found by the review: a limit on a file's size between the old text's and the new's
    // killed the write part way, leaving both names a prefix of the new text, not a
    // document. The file is grown to its new length first, so the limit stops it before a
    // byte of it changes.
    let scratch = Scratch::new("hardlink-limit-mid");
    let box_of = |id: &str, length: u32| {
        format!(
            r#"{{"id": "k7f2qx3{id}", "type": "box", "v": 1, "name": "{id}", "position": [0, 0, 0], "length": {length}, "width": 2, "height": 1}}"#
        )
    };
    let before = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}, {}, {}, {}]}}\n",
        box_of("a", 4),
        box_of("b", 5),
        box_of("c", 6),
        box_of("d", 7)
    );
    assert!(before.len() < 1000, "{}", before.len());
    let a = scratch.file("a.silico", &before);
    let b = scratch.0.join("b.silico");
    std::fs::hard_link(&a, &b).unwrap();
    let warm = run(&scratch, &[os("fmt"), os("--check"), os("a.silico")]);
    assert_eq!(warm.code(), Some(1), "{}", warm.stderr);
    // ulimit -f's block (512 or 1024 bytes, by shell), measured: a limit of 1024 bytes,
    // above the old text, below the new (with four fingerprints).
    let probe = scratch.0.join("probe");
    let _ = Command::new("sh")
        .arg("-c")
        .arg("trap '' XFSZ; ulimit -f 1; head -c 4096 /dev/zero > \"$0\"")
        .arg(&probe)
        .status();
    let block = std::fs::metadata(&probe).unwrap().len();
    std::fs::remove_file(&probe).unwrap();
    let silico = command(&scratch, &worker(), &[]);
    let mut limited = Command::new("sh");
    limited
        .arg("-c")
        .arg(format!(
            "ulimit -f {}; exec \"$0\" fmt a.silico",
            1024 / block
        ))
        .arg(silico.get_program())
        .current_dir(&scratch.0)
        .stdin(Stdio::null());
    for (key, value) in silico.get_envs() {
        match value {
            Some(value) => limited.env(key, value),
            None => limited.env_remove(key),
        };
    }
    // `exec`, so the shell's process is silico's: its number names the copy it keeps.
    let child = limited
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let out = child.wait_with_output().unwrap();
    // Killed by the limit (SIGXFSZ, 25), in the in-place write: the copy of the old text
    // beside the file says it got that far.
    use std::os::unix::process::ExitStatusExt as _;
    assert_eq!(out.status.signal(), Some(25), "{:?}", out.status);
    let kept = scratch.0.join(format!(".a.silico.fmt-{pid}"));
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), before);
    for name in [&a, &b] {
        assert_eq!(
            std::fs::read_to_string(name).unwrap(),
            before,
            "{}",
            name.display()
        );
    }
    assert_eq!(
        std::fs::metadata(&a).unwrap().ino(),
        std::fs::metadata(&b).unwrap().ino()
    );
}

/// `silico fmt a.silico` under `sh -c "<prefix> ulimit -f 1; exec ..."`.
fn fmt_under_one_block(scratch: &Scratch, prefix: &str) -> (u32, std::process::Output) {
    let silico = command(scratch, &worker(), &[]);
    let mut limited = Command::new("sh");
    limited
        .arg("-c")
        .arg(format!("{prefix} ulimit -f 1; exec \"$0\" fmt a.silico"))
        .arg(silico.get_program())
        .current_dir(&scratch.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in silico.get_envs() {
        match value {
            Some(value) => limited.env(key, value),
            None => limited.env_remove(key),
        };
    }
    let child = limited.spawn().unwrap();
    let pid = child.id();
    (pid, child.wait_with_output().unwrap())
}

fn padded(scratch: &Scratch) -> (PathBuf, PathBuf, String) {
    let pad = 2000 - EXAMPLE.len();
    let before = EXAMPLE.replace(
        r#""name": "Block""#,
        &format!(r#""name": "Block{}""#, "x".repeat(pad)),
    );
    assert_eq!(before.len(), 2000);
    let a = scratch.file("a.silico", &before);
    let b = scratch.0.join("b.silico");
    std::fs::hard_link(&a, &b).unwrap();
    let warm = run(scratch, &[os("fmt"), os("--check"), os("a.silico")]);
    assert_eq!(warm.code(), Some(1), "{}", warm.stderr);
    (a, b, before)
}

#[test]
fn fmt_whose_copy_a_size_limit_refuses_says_so_and_leaves_nothing() {
    // A limit below the old text's length, SIGXFSZ ignored: the copy of the old text is
    // refused, the document is as it was, the copy is gone, and the advice is the limit's
    // (found by the review: untested).
    let scratch = Scratch::new("proof-copy-refused");
    let (a, b, before) = padded(&scratch);
    let (_, out) = fmt_under_one_block(&scratch, "trap '' XFSZ;");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("couldn't keep its text"), "{stderr}");
    assert!(stderr.contains("ulimit -f"), "{stderr}");
    for name in [&a, &b] {
        assert_eq!(std::fs::read_to_string(name).unwrap(), before);
    }
    assert_eq!(
        ls(&scratch.0),
        [".silico-cache", "a.silico", "b.silico", "cache-home"]
    );
}

#[test]
fn fmt_whose_copy_a_size_limit_kills_leaves_no_part_of_the_old_text() {
    // The same, killed by the limit: the copy is grown first, so the one left beside the
    // file is empty or whole, never a part named as the whole (found by the review).
    let scratch = Scratch::new("proof-copy-killed");
    let (a, b, before) = padded(&scratch);
    let (pid, out) = fmt_under_one_block(&scratch, "");
    use std::os::unix::process::ExitStatusExt as _;
    assert_eq!(out.status.signal(), Some(25), "{:?}", out.status);
    let kept = std::fs::read_to_string(scratch.0.join(format!(".a.silico.fmt-{pid}"))).unwrap();
    // Empty (stopped before a byte) or whole: never a part named as the whole.
    assert!(kept.is_empty() || kept == before, "{} bytes", kept.len());
    for name in [&a, &b] {
        assert_eq!(std::fs::read_to_string(name).unwrap(), before);
    }
}

#[test]
fn fmt_of_a_single_named_document_refused_by_a_size_limit_says_so() {
    // The rename path's write, under a limit: the limit's advice, and nothing left beside
    // the file (found by the review: it advised on permissions).
    let scratch = Scratch::new("proof-single-limit");
    let pad = 2000 - EXAMPLE.len();
    let before = EXAMPLE.replace(
        r#""name": "Block""#,
        &format!(r#""name": "Block{}""#, "x".repeat(pad)),
    );
    let a = scratch.file("a.silico", &before);
    let warm = run(&scratch, &[os("fmt"), os("--check"), os("a.silico")]);
    assert_eq!(warm.code(), Some(1), "{}", warm.stderr);
    let (_, out) = fmt_under_one_block(&scratch, "trap '' XFSZ;");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), before);
    assert!(stderr.contains("ulimit -f"), "{stderr}");
    assert_eq!(ls(&scratch.0), [".silico-cache", "a.silico", "cache-home"]);
}

#[test]
fn fmt_of_a_single_named_document_killed_by_a_size_limit_leaves_no_part_of_the_new_text() {
    // Killed by the limit (SIGXFSZ at its default), no cleanup runs: the new file beside the
    // document was grown first, so it's empty, never a part of the new text (found by the
    // review: dropping the grow left 1,024 bytes of it, with every test green).
    let scratch = Scratch::new("single-killed");
    let pad = 2000 - EXAMPLE.len();
    let before = EXAMPLE.replace(
        r#""name": "Block""#,
        &format!(r#""name": "Block{}""#, "x".repeat(pad)),
    );
    let a = scratch.file("a.silico", &before);
    let warm = run(&scratch, &[os("fmt"), os("--check"), os("a.silico")]);
    assert_eq!(warm.code(), Some(1), "{}", warm.stderr);
    let (pid, out) = fmt_under_one_block(&scratch, "");
    use std::os::unix::process::ExitStatusExt as _;
    assert_eq!(out.status.signal(), Some(25), "{:?}", out.status);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), before);
    let partial = std::fs::read(scratch.0.join(format!(".a.silico.fmt-{pid}"))).unwrap();
    assert!(
        partial.is_empty(),
        "{} bytes of the new text left beside it",
        partial.len()
    );
}
