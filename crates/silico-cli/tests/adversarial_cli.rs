//! break-it for M2.9's `silico check`, `fmt`, and `show`: hostile paths and files (non-UTF-8
//! names, FIFOs, endless streams, symlinks, hard links, read-only files and directories,
//! names that look like options), a file edited while `fmt` rebuilds it, and kernel workers
//! that crash or are missing. Expected values come from the binary's own contract (its exit
//! codes: 0 clean, 1 the document has problems, 2 the command couldn't run; `--json`'s
//! line per file), POSIX file semantics, and what other formatters do (rustfmt, gofmt -w,
//! and prettier --write refuse a read-only file and write through a symlink).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{OsStr, OsString};
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

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

/// Waits for `child` up to `deadline`, killing it after: `status` is `None` if it hung.
fn finish(mut child: Child, deadline: Duration) -> Ran {
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err.read_to_end(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if start.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ran {
        status,
        stdout: String::from_utf8_lossy(&out.join().unwrap()).into_owned(),
        stderr: String::from_utf8_lossy(&err.join().unwrap()).into_owned(),
    }
}

fn run(scratch: &Scratch, args: &[&OsStr]) -> Ran {
    run_with(scratch, &worker(), args)
}

fn run_with(scratch: &Scratch, worker: &Path, args: &[&OsStr]) -> Ran {
    let child = command(scratch, worker, args).spawn().unwrap();
    finish(child, Duration::from_secs(120))
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
    let child = command(&scratch, &worker(), &[os("show"), os("part.silico")])
        .spawn()
        .unwrap();
    let writer = std::thread::spawn(move || {
        let mut f = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
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
    let written = writer.join().unwrap();
    let limit = silico_core::document::json::MAX_INPUT_LEN;
    assert!(out.status.is_some(), "hung");
    assert!(
        written <= limit + (2 << 20),
        "silico read {written} bytes, {} past the {limit} it may read",
        written - limit
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
    if out.code() == Some(2) {
        assert_eq!(real_text, EXAMPLE);
    } else {
        assert_eq!(out.code(), Some(0), "{}", out.stderr);
        assert!(
            link.file_type().is_symlink(),
            "the link was replaced by a file"
        );
        assert!(
            real_text.contains("fingerprint"),
            "the real document wasn't formatted"
        );
    }
    assert!(
        link.file_type().is_symlink(),
        "the link was replaced by a file"
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
    if out.code() == Some(2) {
        assert_eq!(std::fs::read_to_string(&a).unwrap(), EXAMPLE);
        return;
    }
    assert_eq!(out.code(), Some(0), "{}", out.stderr);
    assert_eq!(
        std::fs::metadata(&a).unwrap().ino(),
        std::fs::metadata(&b).unwrap().ino(),
        "the hard link was split"
    );
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
    // user's editor, an agent, `git checkout`) is lost without a word, exit 0. Here a
    // worker that takes 3 s to start stands in for a long rebuild, and the edit (the block
    // 41 long) lands after 1 s. Expected: the edit kept: fmt sees the file changed since it
    // read it and refuses (exit 2, "changed while it was formatted") or formats the new
    // text.
    let scratch = Scratch::new("race");
    let file = scratch.file("part.silico", EXAMPLE);
    let slow = script(
        &scratch,
        "slow-worker",
        &format!("sleep 3\nexec '{}' \"$@\"", worker().display()),
    );
    let child = command(&scratch, &slow, &[os("fmt"), os("part.silico")])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let edited = EXAMPLE.replace(r#""length": 40"#, r#""length": 41"#);
    std::fs::write(&file, &edited).unwrap();
    let out = finish(child, Duration::from_secs(120));
    let now = std::fs::read_to_string(&file).unwrap();
    assert!(
        now.contains(r#""length": 41"#),
        "the edit was lost (exit {:?}): {now}\n{}",
        out.code(),
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
    let (a, b) = (
        finish(a, Duration::from_secs(120)),
        finish(b, Duration::from_secs(120)),
    );
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
    // BREAKS: a worker that exits as soon as it starts makes every feature fail with
    // worker_crashed, and `check` calls that a problem with the document: exit 1 and
    // `{"ok": false}`, so an agent "fixes" a sound document; `fmt` prints the crash and
    // exits 0, after rewriting the file without the fingerprints it promised. The binary's
    // contract: 1 is "a document has problems", 2 is "a kernel that couldn't start".
    let scratch = Scratch::new("crash");
    let file = scratch.file("part.silico", EXAMPLE);
    let dies = script(&scratch, "dies", "exit 3");
    let out = run_with(&scratch, &dies, &[os("check"), os("part.silico")]);
    assert!(out.stderr.contains("worker_crashed"), "{}", out.stderr);
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
