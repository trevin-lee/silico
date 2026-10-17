//! The `silico` command-line client: `check`, `fmt`, and `show`, each a thin client of the
//! core's commands (`silico_core::command`), which the app and MCP run too, and `textconv`,
//! for `git diff` (docs/cli.md).
//!
//! Exit codes: 0, nothing wrong; 1, a document has problems (or, with `fmt --check`, isn't
//! in canonical form); 2, the command couldn't run (a usage error, a file that couldn't be
//! read or written, a kernel that couldn't start or failed in a way that isn't the
//! document's).

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::command::{self, Problems};
use silico_core::document::Checked;
use silico_core::document::json::MAX_INPUT_LEN;
use silico_core::pool::KernelPool;
use silico_core::recompute::Model;
use silico_core::recompute::Recompute;
use silico_core::{Error, ErrorCode};

/// A line on stderr (the workspace forbids `eprintln!`, which panics if stderr is closed).
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr().lock(), $($arg)*);
    }};
}

const USAGE: &str = "usage: silico check [--json] [--] FILE...
       silico fmt [--check] [--] FILE...
       silico show [--] FILE
       silico textconv [--] FILE";

/// How a command ended.
enum Ended {
    /// Nothing wrong.
    Clean,
    /// A document has problems, or isn't in canonical form.
    Problems,
    /// The command couldn't run.
    Failed,
}

impl Ended {
    fn code(&self) -> ExitCode {
        match self {
            Ended::Clean => ExitCode::SUCCESS,
            Ended::Problems => ExitCode::from(1),
            Ended::Failed => ExitCode::from(2),
        }
    }

    /// The worse of two.
    fn and(self, other: Ended) -> Ended {
        match (self, other) {
            (Ended::Failed, _) | (_, Ended::Failed) => Ended::Failed,
            (Ended::Problems, _) | (_, Ended::Problems) => Ended::Problems,
            (Ended::Clean, Ended::Clean) => Ended::Clean,
        }
    }
}

/// A command, as named, with its options.
enum Command {
    Check { json: bool },
    Fmt { check: bool },
    Show,
    Textconv,
}

impl Command {
    /// The command `name` with `flags`; or, for a command or an option there's no such
    /// thing as, what to say.
    fn parse(name: &str, flags: &[String]) -> Result<Command, String> {
        let flag = |option: &str| flags.iter().any(|f| f == option);
        let (command, options): (Command, &[&str]) = match name {
            "check" => (
                Command::Check {
                    json: flag("--json"),
                },
                &["--json"],
            ),
            "fmt" => (
                Command::Fmt {
                    check: flag("--check"),
                },
                &["--check"],
            ),
            "show" => (Command::Show, &[]),
            "textconv" => (Command::Textconv, &[]),
            _ => return Err(format!("silico: no command {name:?}")),
        };
        match flags.iter().find(|f| !options.contains(&f.as_str())) {
            Some(unknown) => Err(format!("silico {name}: no option {unknown:?}")),
            None => Ok(command),
        }
    }
}

fn main() -> ExitCode {
    silico_core::logging::init();
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "silico starting");
    // As the OS gives them: a file's name needn't be UTF-8.
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some((name, rest)) = args.split_first() else {
        say!("{USAGE}");
        return Ended::Failed.code();
    };
    // Options, then files; `--` ends the options, so a file may be named like one.
    let mut flags: Vec<String> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut options = true;
    for arg in rest {
        if options && arg == "--" {
            options = false;
        } else if options && arg.to_string_lossy().starts_with("--") {
            flags.push(arg.to_string_lossy().into_owned());
        } else {
            files.push(PathBuf::from(arg));
        }
    }
    let command = match Command::parse(&name.to_string_lossy(), &flags) {
        Ok(command) => command,
        Err(why) => {
            say!("{why}\n{USAGE}");
            return Ended::Failed.code();
        }
    };
    let mut engines = Engines::default();
    let ended = match (command, files.as_slice()) {
        (_, []) | (Command::Show | Command::Textconv, [_, _, ..]) => {
            say!("{USAGE}");
            Ended::Failed
        }
        (Command::Check { json }, files) => files
            .iter()
            .map(|file| check(&mut engines, file, json))
            .fold(Ended::Clean, Ended::and),
        (Command::Fmt { check }, files) => files
            .iter()
            .map(|file| fmt(&mut engines, file, check))
            .fold(Ended::Clean, Ended::and),
        (Command::Show, [file]) => show(file),
        (Command::Textconv, [file]) => textconv(file),
    };
    ended.code()
}

/// `silico check FILE`: its problems, each on stderr; or, with `--json`, one line of JSON
/// on stdout for each file: `{"file", "ok", "problems", "total"}`, each problem an error as
/// the core's error contract has it, or, for a file that couldn't be checked,
/// `{"file", "ok": false, "error"}`, the error that stopped it, in the same shape
/// (tests/contract/check-line.json).
fn check(engines: &mut Engines, file: &Path, json: bool) -> Ended {
    let failed = |error: Error| {
        if json {
            line(serde_json::json!({"file": shown(file), "ok": false, "error": error}));
        } else {
            say!("{}: {error}", shown(file));
        }
        Ended::Failed
    };
    let bytes = match read(file) {
        Ok(bytes) => bytes,
        Err(error) => return failed(error),
    };
    // What's wrong with the document needs no kernel: said even when none can start.
    let problems = match command::gate(&bytes) {
        Err(problems) => problems,
        Ok(checked) => match engines
            .rebuild(file, checked)
            .and_then(|model| command::check(&model))
        {
            Ok(problems) => problems,
            Err(error) => return failed(error),
        },
    };
    if json {
        line(serde_json::json!({
            "file": shown(file),
            "ok": problems.is_empty(),
            "problems": problems.errors(),
            "total": problems.total(),
        }));
    } else {
        print_problems(file, &problems);
    }
    if problems.is_empty() {
        Ended::Clean
    } else {
        Ended::Problems
    }
}

/// `silico fmt FILE`: rewrites the file (the one it names, through a symbolic link) in
/// canonical form with its fingerprints as rebuilt, then says on stderr which fingerprints
/// it wrote, and the problems it couldn't fix (a feature that didn't build keeps the
/// fingerprint stored with it, and the file is written); with `--check`, writes nothing,
/// and says whether it would. A read-only file, one changed while it was rebuilt, or one
/// whose rebuild failed for the kernel's own reasons (a worker that crashed or hung), isn't
/// written.
fn fmt(engines: &mut Engines, file: &Path, check_only: bool) -> Ended {
    let bytes = match read(file) {
        Ok(bytes) => bytes,
        Err(error) => {
            say!("{}: {error}", shown(file));
            return Ended::Failed;
        }
    };
    let checked = match command::gate(&bytes) {
        Ok(checked) => checked,
        Err(problems) => {
            print_problems(file, &problems);
            return Ended::Problems;
        }
    };
    let formatted = match engines
        .rebuild(file, checked)
        .and_then(|model| command::format(&model))
    {
        Ok(Ok(formatted)) => formatted,
        Ok(Err(problems)) => {
            print_problems(file, &problems);
            return Ended::Problems;
        }
        Err(error) => {
            say!("{}: {error}", shown(file));
            return Ended::Failed;
        }
    };
    print_problems(file, &formatted.problems);
    let problems = if formatted.problems.is_empty() {
        Ended::Clean
    } else {
        Ended::Problems
    };
    let same = formatted.text.as_bytes() == bytes.as_slice();
    if check_only {
        if same {
            return problems;
        }
        say!(
            "{}: not in canonical form; `silico fmt` would rewrite it",
            shown(file)
        );
        return Ended::Problems;
    }
    if !same && let Err(error) = write_whole(file, &bytes, formatted.text.as_bytes()) {
        say!("{}: {error}", shown(file));
        return Ended::Failed;
    }
    for (id, before) in &formatted.refingerprinted {
        let what = if before.is_some() { "rewrote" } else { "wrote" };
        say!("{}: {what} the fingerprint of {id}", shown(file));
    }
    problems
}

/// The engine the last file was rebuilt on, and the directory of its cache: the next file
/// whose cache is there is rebuilt on it too, so `silico check *.silico` starts its kernel
/// workers once, not once a file.
#[derive(Default)]
struct Engines {
    current: Option<(PathBuf, Recompute<KernelPool>)>,
}

impl Engines {
    /// `checked`, read from `file`, rebuilt: on kernel workers, with the rebuild cache
    /// beside the file (`.silico-cache/`, beside the file a link names).
    fn rebuild(&mut self, file: &Path, checked: Checked) -> Result<Arc<Model>, Error> {
        let dir = cache_directory(file);
        // Another directory's engine is stopped before this one's starts.
        let kept = self.current.take().filter(|(of, _)| *of == dir);
        let (dir, engine) = match kept {
            Some(current) => current,
            None => {
                let engine = Recompute::with_cache(
                    KernelPool::start()?,
                    Cache::open(dir.join(".silico-cache")),
                );
                (dir, engine)
            }
        };
        let engine = &self.current.insert((dir, engine)).1;
        command::rebuild(engine, checked)?.ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                "the rebuild was superseded, though nothing else uses its engine",
            )
            .with_suggestion(silico_core::worker::SILICO_BUG)
        })
    }
}

/// Where `file`'s rebuild cache is: beside the file it names (through a link), so every
/// name of a document shares one.
fn cache_directory(file: &Path) -> PathBuf {
    let named = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned());
    directory(&named).to_owned()
}

/// `silico show FILE`: its outline, on stdout. Needs no kernel: it's the document as
/// written, for reading and for `git diff`.
fn show(file: &Path) -> Ended {
    let bytes = match read(file) {
        Ok(bytes) => bytes,
        Err(error) => {
            say!("{}: {error}", shown(file));
            return Ended::Failed;
        }
    };
    match silico_core::document::read(&bytes) {
        Ok(document) => {
            let mut out = std::io::stdout().lock();
            if out
                .write_all(command::outline(&document).as_bytes())
                .and_then(|()| out.flush())
                .is_err()
            {
                return Ended::Failed;
            }
            Ended::Clean
        }
        Err(error) => {
            say!("{}: {error}", shown(file));
            Ended::Problems
        }
    }
}

/// `silico textconv FILE`, git's text conversion for `*.silico` (docs/cli.md): the outline,
/// as `show` prints it, if the file reads as a document; if not (a merge's conflict
/// markers, an edit half made), the file as it is, so `git diff` still shows it. Fails only
/// if the file can't be read or stdout written.
fn textconv(file: &Path) -> Ended {
    let fail = |error: Error| {
        say!("{}: {error}", shown(file));
        Ended::Failed
    };
    let (mut opened, bytes) = match opened_and_read(file) {
        Ok(read) => read,
        Err(error) => return fail(error),
    };
    let mut out = std::io::stdout().lock();
    let written = match silico_core::document::read(&bytes) {
        Ok(document) => out.write_all(command::outline(&document).as_bytes()),
        // As it is, all of it: what was read, then the rest of a file too long to read.
        Err(_) => out
            .write_all(&bytes)
            .and_then(|()| std::io::copy(&mut opened, &mut out).map(drop)),
    };
    match written.and_then(|()| out.flush()) {
        Ok(()) => Ended::Clean,
        Err(e) => fail(io_failed(format!("couldn't be converted: {e}"))),
    }
}

/// A file's name as it's shown (its bytes, any that aren't UTF-8 replaced).
fn shown(file: &Path) -> String {
    file.display().to_string()
}

fn line(value: serde_json::Value) {
    let _ = writeln!(std::io::stdout().lock(), "{value}");
}

/// `file`'s bytes, if it's a regular file (a document is one: a pipe or a device, which
/// could hang or never end, isn't), at most one more than a document may be (the reader
/// refuses it then, saying so); or why not.
fn read(file: &Path) -> Result<Vec<u8>, Error> {
    opened_and_read(file).map(|(_, bytes)| bytes)
}

/// `file`, opened, and as much of it as [`read`] reads, the rest left to read.
fn opened_and_read(file: &Path) -> Result<(std::fs::File, Vec<u8>), Error> {
    let couldnt = |e: std::io::Error| io_failed(format!("couldn't be read: {e}"));
    // Opened without waiting (a pipe with no writer would block an open), then checked.
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file)
        .map_err(couldnt)?;
    let metadata = opened.metadata().map_err(couldnt)?;
    if !metadata.is_file() {
        return Err(io_failed(
            "couldn't be read: it isn't a regular file".to_owned(),
        ));
    }
    let mut opened = opened;
    let mut bytes = Vec::new();
    (&mut opened)
        .take(MAX_INPUT_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(couldnt)?;
    Ok((opened, bytes))
}

fn directory(file: &Path) -> &Path {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Each problem on stderr, after the file it's about: its code and words, then the
/// feature, elements, and parameter at fault, and what to do (the error's own `Display`);
/// and how many more there are, if not all are listed.
fn print_problems(file: &Path, problems: &Problems) {
    for error in problems.errors() {
        say!("{}: {error}", shown(file));
    }
    let listed = problems.errors().len() as u64;
    if problems.total() > listed {
        say!(
            "{}: and {} more problems",
            shown(file),
            problems.total() - listed
        );
    }
}

/// A file that couldn't be read or written, as `why` says.
fn io_failed(why: String) -> Error {
    let suggestion = if why.starts_with("couldn't be read") {
        "check the file's name, and that it's a regular file this user may read"
    } else if why.starts_with("couldn't be written") {
        "check that this user may write the file and its directory, and that the disk has room"
    } else {
        "check that the output can be written (a closed pipe stops it)"
    };
    Error::new(ErrorCode::IoFailed, why).with_suggestion(suggestion)
}

/// Writes `bytes` over `file`, which held `before` when it was read: the file it names
/// (through a symbolic link, which stays), keeping its permissions. Whole, to a new file
/// beside it renamed over it, so a reader never sees a part of it and a write cut short
/// leaves the old file; or, if it has other names (hard links), which a rename would part
/// from it, in place, with the old text kept beside it until the new is on disk, and put
/// back if the write fails (a full disk), so a write cut short never loses the document
/// (killed mid-write, the old text is in the file beside it, `.NAME.fmt-PID`). Refused if
/// this process may not write it, or it changed since it was read.
fn write_whole(file: &Path, before: &[u8], bytes: &[u8]) -> Result<(), Error> {
    let target =
        std::fs::canonicalize(file).map_err(|e| io_failed(format!("couldn't be written: {e}")))?;
    let metadata =
        std::fs::metadata(&target).map_err(|e| io_failed(format!("couldn't be written: {e}")))?;
    // Whether this process may write it, whatever its mode says (a file its owner can't
    // write but others can, another user's): opened to write, not truncated.
    let writable = std::fs::OpenOptions::new().write(true).open(&target);
    let mut writable = match writable {
        Ok(_) if metadata.permissions().readonly() => None,
        Ok(writable) => Some(writable),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => None,
        Err(e) => return Err(io_failed(format!("couldn't be written: {e}"))),
    }
    .ok_or_else(|| io_failed("couldn't be written: it's read-only".to_owned()))?;
    // Changed while it was rebuilt (another editor saved it): left to them.
    if read(&target)? != before {
        return Err(io_failed(
            "couldn't be written: it changed while it was rebuilt; run `silico fmt` again"
                .to_owned(),
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
        io_failed(format!("couldn't be written: {why}"))
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
    if let Err(e) = copy.write_all(before).and_then(|()| copy.sync_all()) {
        let _ = std::fs::remove_file(&kept);
        return Err(io_failed(format!(
            "couldn't be written: couldn't keep its text in {}: {e}",
            kept.display()
        )));
    }
    let over = |file: &mut std::fs::File, text: &[u8]| {
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
                Err(io_failed(format!(
                    "couldn't be written: {e}; it's as it was"
                )))
            }
            Err(_) => Err(io_failed(format!(
                "couldn't be written: {e}; its text before is in {}",
                kept.display()
            ))),
        },
    }
}

/// A new file beside `target` (`.NAME.fmt-PID`, or `-1`, `-2`, ... after it if one is
/// left from a run killed with the same process number), no more open to others than
/// `target` is.
fn beside(target: &Path, metadata: &std::fs::Metadata) -> Result<(PathBuf, std::fs::File), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = directory(target);
    let name = target
        .file_name()
        .ok_or_else(|| io_failed("couldn't be written: it has no file name".to_owned()))?;
    let mut last = None;
    for n in 0..64 {
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
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => {
                return Err(io_failed(format!(
                    "couldn't be written: couldn't create a file in {}: {e}",
                    dir.display()
                )));
            }
        }
    }
    Err(io_failed(format!(
        "couldn't be written: couldn't create a file in {}: {}",
        dir.display(),
        last.map_or_else(String::new, |e| e.to_string())
    )))
}
