//! The `silico` command-line client: `check`, `fmt`, and `show`, each a thin client of the
//! core's commands (`silico_core::command`), which the app and MCP run too.
//!
//! Exit codes: 0, nothing wrong; 1, a document has problems (or, with `fmt --check`, isn't
//! in canonical form); 2, the command couldn't run (a usage error, a file that couldn't be
//! read or written, a kernel that couldn't start).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use silico_core::cache::Cache;
use silico_core::command::{self, Problems};
use silico_core::pool::KernelPool;
use silico_core::recompute::Recompute;
use silico_core::{Error, ErrorCode};

/// A line on stderr (the workspace forbids `eprintln!`, which panics if stderr is closed).
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr().lock(), $($arg)*);
    }};
}

const USAGE: &str = "usage: silico check [--json] FILE...
       silico fmt [--check] FILE...
       silico show FILE";

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        say!("{USAGE}");
        return Ended::Failed.code();
    };
    let (flags, files): (Vec<&String>, Vec<&String>) =
        rest.iter().partition(|a| a.starts_with("--"));
    let flag = |name: &str| flags.iter().any(|f| f.as_str() == name);
    let known: &[&str] = match command.as_str() {
        "check" => &["--json"],
        "fmt" => &["--check"],
        "show" => &[],
        _ => {
            say!("silico: no command {:?}\n{USAGE}", command);
            return Ended::Failed.code();
        }
    };
    if let Some(unknown) = flags.iter().find(|f| !known.contains(&f.as_str())) {
        say!("silico {command}: no option {unknown:?}\n{USAGE}");
        return Ended::Failed.code();
    }
    if files.is_empty() || (command == "show" && files.len() != 1) {
        say!("{USAGE}");
        return Ended::Failed.code();
    }
    let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let ended = match command.as_str() {
        "check" => files
            .iter()
            .map(|file| check(file, flag("--json")))
            .fold(Ended::Clean, Ended::and),
        "fmt" => files
            .iter()
            .map(|file| fmt(file, flag("--check")))
            .fold(Ended::Clean, Ended::and),
        _ => show(&files[0]),
    };
    ended.code()
}

/// `silico check FILE`: its problems, each on stderr (or, with `--json`, a line of JSON on
/// stdout: `{"file", "ok", "problems", "total"}`, each problem an error as the core's
/// error contract has it).
fn check(file: &Path, json: bool) -> Ended {
    let Some(bytes) = read(file) else {
        return Ended::Failed;
    };
    let Some(engine) = engine(file) else {
        return Ended::Failed;
    };
    let problems = match command::check(&engine, &bytes) {
        Ok(problems) => problems,
        Err(error) => {
            report(file, &error);
            return Ended::Failed;
        }
    };
    if json {
        let line = serde_json::json!({
            "file": file.display().to_string(),
            "ok": problems.is_empty(),
            "problems": problems.problems,
            "total": problems.total,
        });
        let _ = writeln!(std::io::stdout().lock(), "{line}");
    } else {
        print_problems(file, &problems);
    }
    if problems.is_empty() {
        Ended::Clean
    } else {
        Ended::Problems
    }
}

/// `silico fmt FILE`: rewrites it in canonical form with its fingerprints as rebuilt, whole
/// (to a file beside it, then renamed over it), saying on stderr which fingerprints it
/// wrote; with `--check`, writes nothing, and says whether it would.
fn fmt(file: &Path, check_only: bool) -> Ended {
    let Some(bytes) = read(file) else {
        return Ended::Failed;
    };
    let Some(engine) = engine(file) else {
        return Ended::Failed;
    };
    let formatted = match command::format(&engine, &bytes) {
        Ok(Ok(formatted)) => formatted,
        Ok(Err(problems)) => {
            print_problems(file, &problems);
            return Ended::Problems;
        }
        Err(error) => {
            report(file, &error);
            return Ended::Failed;
        }
    };
    for error in &formatted.problems {
        report(file, error);
    }
    let same = formatted.text.as_bytes() == bytes.as_slice();
    if check_only {
        if same {
            return Ended::Clean;
        }
        say!(
            "{}: not in canonical form; `silico fmt` would rewrite it",
            file.display()
        );
        return Ended::Problems;
    }
    for (id, before) in &formatted.refingerprinted {
        let what = if before.is_some() { "rewrote" } else { "wrote" };
        say!("{}: {what} the fingerprint of {id}", file.display());
    }
    if same {
        return Ended::Clean;
    }
    match write_whole(file, formatted.text.as_bytes()) {
        Ok(()) => Ended::Clean,
        Err(e) => {
            say!("{}: couldn't be written: {e}", file.display());
            Ended::Failed
        }
    }
}

/// `silico show FILE`: its outline, on stdout. Needs no kernel: it's the document as
/// written, for reading and for `git diff`.
fn show(file: &Path) -> Ended {
    let Some(bytes) = read(file) else {
        return Ended::Failed;
    };
    match silico_core::document::read(&bytes) {
        Ok(document) => {
            let mut out = std::io::stdout().lock();
            if out
                .write_all(command::outline(&document).as_bytes())
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

/// `file`'s bytes, or `None`, said why on stderr.
fn read(file: &Path) -> Option<Vec<u8>> {
    match std::fs::read(file) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            say!("{}: couldn't be read: {e}", file.display());
            None
        }
    }
}

/// An engine for `file`: kernel workers, and the rebuild cache beside it
/// (`.silico-cache/`).
fn engine(file: &Path) -> Option<Recompute<KernelPool>> {
    let pool = match KernelPool::start() {
        Ok(pool) => pool,
        Err(error) => {
            report(file, &error);
            return None;
        }
    };
    let dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Some(Recompute::with_cache(
        pool,
        Cache::open(dir.join(".silico-cache")),
    ))
}

fn print_problems(file: &Path, problems: &Problems) {
    for error in &problems.problems {
        report(file, error);
    }
    let listed = problems.problems.len() as u64;
    if problems.total > listed {
        say!(
            "{}: and {} more problems",
            file.display(),
            problems.total - listed
        );
    }
}

/// An error on stderr, after the file it's about: its code and words, then the feature,
/// elements, and parameter at fault, and what to do (the error's own `Display`).
fn report(file: &Path, error: &Error) {
    say!("{}: {}: {error}", file.display(), code(error.code()));
}

fn code(code: ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{code:?}"))
}

/// Writes `bytes` to `file` whole: to a new file beside it, then renamed over it, so a
/// reader never sees a part of it and a write cut short leaves the old file.
fn write_whole(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = file
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?;
    let mut partial_name = std::ffi::OsString::from(".");
    partial_name.push(name);
    partial_name.push(format!(".fmt-{}", std::process::id()));
    let partial = dir.join(partial_name);
    let permissions = std::fs::metadata(file)?.permissions();
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.set_permissions(permissions)?;
            f.sync_all()
        });
    let renamed = written.and_then(|()| std::fs::rename(&partial, file));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    renamed
}
