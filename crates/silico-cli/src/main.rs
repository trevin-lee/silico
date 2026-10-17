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

use silico_core::Error;
use silico_core::cache::Cache;
use silico_core::command::{self, Problems};
use silico_core::document::Checked;
use silico_core::document::json::MAX_INPUT_LEN;
use silico_core::pool::KernelPool;
use silico_core::recompute::Recompute;

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

fn main() -> ExitCode {
    silico_core::logging::init();
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "silico starting");
    // As the OS gives them: a file's name needn't be UTF-8.
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        say!("{USAGE}");
        return Ended::Failed.code();
    };
    let command = command.to_string_lossy().into_owned();
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
    let flag = |name: &str| flags.iter().any(|f| f == name);
    let known: &[&str] = match command.as_str() {
        "check" => &["--json"],
        "fmt" => &["--check"],
        "show" | "textconv" => &[],
        _ => {
            say!("silico: no command {command:?}\n{USAGE}");
            return Ended::Failed.code();
        }
    };
    if let Some(unknown) = flags.iter().find(|f| !known.contains(&f.as_str())) {
        say!("silico {command}: no option {unknown:?}\n{USAGE}");
        return Ended::Failed.code();
    }
    if files.is_empty() || (matches!(command.as_str(), "show" | "textconv") && files.len() != 1) {
        say!("{USAGE}");
        return Ended::Failed.code();
    }
    let ended = match command.as_str() {
        "check" => files
            .iter()
            .map(|file| check(file, flag("--json")))
            .fold(Ended::Clean, Ended::and),
        "fmt" => files
            .iter()
            .map(|file| fmt(file, flag("--check")))
            .fold(Ended::Clean, Ended::and),
        "show" => files.first().map_or(Ended::Failed, |file| show(file)),
        _ => files.first().map_or(Ended::Failed, |file| textconv(file)),
    };
    ended.code()
}

/// `silico check FILE`: its problems, each on stderr; or, with `--json`, one line of JSON
/// on stdout for each file: `{"file", "ok", "problems", "total"}`, each problem an error as
/// the core's error contract has it, or, for a file that couldn't be checked,
/// `{"file", "ok": false, "error"}`, the error in words.
fn check(file: &Path, json: bool) -> Ended {
    let failed = |why: String| {
        if json {
            line(serde_json::json!({"file": shown(file), "ok": false, "error": why}));
        } else {
            say!("{}: {why}", shown(file));
        }
        Ended::Failed
    };
    let bytes = match read(file) {
        Ok(bytes) => bytes,
        Err(why) => return failed(why),
    };
    // What's wrong with the document needs no kernel: said even when none can start.
    let problems = match command::gate(&bytes) {
        Err(problems) => problems,
        Ok(checked) => {
            let engine = match engine(file) {
                Ok(engine) => engine,
                Err(error) => return failed(error.to_string()),
            };
            match command::check(&engine, checked) {
                Ok(problems) => problems,
                Err(error) => return failed(error.to_string()),
            }
        }
    };
    if json {
        line(serde_json::json!({
            "file": shown(file),
            "ok": problems.is_empty(),
            "problems": problems.problems,
            "total": problems.total,
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
/// it wrote, and the problems it couldn't fix (a feature that didn't build keeps its
/// fingerprint); with `--check`, writes nothing, and says whether it would. A read-only
/// file, one changed while it was rebuilt, or one the kernel couldn't rebuild, isn't
/// written.
fn fmt(file: &Path, check_only: bool) -> Ended {
    let bytes = match read(file) {
        Ok(bytes) => bytes,
        Err(why) => {
            say!("{}: {why}", shown(file));
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
    let formatted = match rebuild_formatted(file, checked) {
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
    let problems = if formatted.problems.is_empty() {
        Ended::Clean
    } else {
        Ended::Problems
    };
    for error in &formatted.problems {
        report(file, error);
    }
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
    if !same && let Err(why) = write_whole(file, &bytes, formatted.text.as_bytes()) {
        say!("{}: couldn't be written: {why}", shown(file));
        return Ended::Failed;
    }
    for (id, before) in &formatted.refingerprinted {
        let what = if before.is_some() { "rewrote" } else { "wrote" };
        say!("{}: {what} the fingerprint of {id}", shown(file));
    }
    problems
}

fn rebuild_formatted(
    file: &Path,
    checked: Checked,
) -> Result<Result<command::Formatted, Problems>, Error> {
    command::format(&engine(file)?, checked)
}

/// `silico show FILE`: its outline, on stdout. Needs no kernel: it's the document as
/// written, for reading and for `git diff`.
fn show(file: &Path) -> Ended {
    let bytes = match read(file) {
        Ok(bytes) => bytes,
        Err(why) => {
            say!("{}: {why}", shown(file));
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
            report(file, &error);
            Ended::Problems
        }
    }
}

/// `silico textconv FILE`, git's text conversion for `*.silico` (docs/cli.md): the outline,
/// as `show` prints it, if the file reads as a document; if not (a merge's conflict
/// markers, an edit half made), the file as it is, so `git diff` still shows it. Fails only
/// if the file can't be read or stdout written.
fn textconv(file: &Path) -> Ended {
    let fail = |why: String| {
        say!("{}: {why}", shown(file));
        Ended::Failed
    };
    let mut opened = match open(file) {
        Ok(opened) => opened,
        Err(why) => return fail(why),
    };
    let mut bytes = Vec::new();
    if let Err(e) = (&mut opened)
        .take(MAX_INPUT_LEN as u64 + 1)
        .read_to_end(&mut bytes)
    {
        return fail(format!("couldn't be read: {e}"));
    }
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
        Err(e) => fail(format!("couldn't be converted: {e}")),
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
fn read(file: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    open(file)?
        .take(MAX_INPUT_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("couldn't be read: {e}"))?;
    Ok(bytes)
}

/// `file`, opened to read, if it's a regular file; or why not.
fn open(file: &Path) -> Result<std::fs::File, String> {
    let couldnt = |e: std::io::Error| format!("couldn't be read: {e}");
    // Opened without waiting (a pipe with no writer would block an open), then checked.
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(file)
        .map_err(couldnt)?;
    let metadata = opened.metadata().map_err(couldnt)?;
    if !metadata.is_file() {
        return Err("couldn't be read: it isn't a regular file".to_owned());
    }
    Ok(opened)
}

/// An engine for `file`: kernel workers, and the rebuild cache beside it
/// (`.silico-cache/`).
fn engine(file: &Path) -> Result<Recompute<KernelPool>, Error> {
    let pool = KernelPool::start()?;
    Ok(Recompute::with_cache(
        pool,
        Cache::open(directory(file).join(".silico-cache")),
    ))
}

fn directory(file: &Path) -> &Path {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn print_problems(file: &Path, problems: &Problems) {
    for error in &problems.problems {
        report(file, error);
    }
    let listed = problems.problems.len() as u64;
    if problems.total > listed {
        say!(
            "{}: and {} more problems",
            shown(file),
            problems.total - listed
        );
    }
}

/// An error on stderr, after the file it's about: its code and words, then the feature,
/// elements, and parameter at fault, and what to do (the error's own `Display`).
fn report(file: &Path, error: &Error) {
    say!("{}: {error}", shown(file));
}

/// Writes `bytes` over `file`, which held `before` when it was read: the file it names
/// (through a symbolic link, which stays), whole (to a new file beside it, then renamed
/// over it, so a reader never sees a part of it and a write cut short leaves the old
/// file), keeping its permissions; or in place, if it has other names (hard links), which
/// a rename would part from it. Refused if it's read-only, or changed since it was read.
fn write_whole(file: &Path, before: &[u8], bytes: &[u8]) -> Result<(), String> {
    let target = std::fs::canonicalize(file).map_err(|e| e.to_string())?;
    let metadata = std::fs::metadata(&target).map_err(|e| e.to_string())?;
    if metadata.permissions().readonly() {
        return Err("it's read-only".to_owned());
    }
    // Changed while it was rebuilt (another editor saved it): left to them.
    if read(&target)? != before {
        return Err("it changed while it was rebuilt; run `silico fmt` again".to_owned());
    }
    if metadata.nlink() > 1 {
        return std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&target)
            .and_then(|mut f| {
                f.write_all(bytes)?;
                f.sync_all()
            })
            .map_err(|e| e.to_string());
    }
    let dir = directory(&target);
    let name = target.file_name().ok_or("it has no file name")?;
    let mut partial_name = OsString::from(".");
    partial_name.push(name);
    partial_name.push(format!(".fmt-{}", std::process::id()));
    let partial = dir.join(partial_name);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.set_permissions(metadata.permissions())?;
            f.sync_all()
        });
    let renamed = written.and_then(|()| std::fs::rename(&partial, &target));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    renamed.map_err(|e| e.to_string())
}
