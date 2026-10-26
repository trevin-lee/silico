//! The `silico` command-line client: `check`, `fmt`, `show`, and `schema`, each a thin client
//! of the core's commands (`silico_core::command`), which MCP and the app run too, and
//! `textconv`, for `git diff` (docs/cli.md). `check`, `fmt`, and `show` each open a file as a
//! session, run one session command on it (`check`, `format`, `outline`), and close it
//! (docs/design/m4.1-command-api.md, section 9), every file on one kernel pool.
//!
//! Exit codes: 0, nothing wrong; 1, a document has problems (or, with `fmt --check`, isn't
//! in canonical form); 2, the command couldn't run (a usage error, a file that couldn't be
//! read or written, a kernel that couldn't start or failed in a way that isn't the
//! document's).

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use silico_core::cache::Cache;
use silico_core::command::{self, ModelChoice, Problems, Response, SessionId, Sessions};
use silico_core::file::opened_and_read;
use silico_core::kernel::Clients;
use silico_core::pool::KernelPool;
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
       silico textconv [--] FILE
       silico schema [NAME]";

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
    Schema,
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
            "schema" => (Command::Schema, &[]),
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
    let ended = match (command, files.as_slice()) {
        (Command::Schema, []) => schema("document"),
        (Command::Schema, [name]) => schema(&name.to_string_lossy()),
        (_, []) | (Command::Show | Command::Textconv | Command::Schema, [_, _, ..]) => {
            say!("{USAGE}");
            Ended::Failed
        }
        (Command::Check { json }, files) => rebuilding(&Rebuilding::Check { json }, files),
        (Command::Fmt { check }, files) => rebuilding(&Rebuilding::Fmt { check }, files),
        (Command::Show, [file]) => show(file),
        (Command::Textconv, [file]) => textconv(file),
    };
    ended.code()
}

/// `silico schema [NAME]`: the document's JSON Schema, or the command `NAME`'s request and
/// result schemas, as the core's `schema` gives them and its committed snapshots hold them.
fn schema(of: &str) -> Ended {
    let text = match command::schema::schema(of) {
        Ok(described) => described.text(),
        Err(problems) => {
            for error in problems.errors() {
                say!("silico schema: {error}");
            }
            return Ended::Failed;
        }
    };
    match text {
        Ok(text) => {
            let mut out = std::io::stdout().lock();
            if out
                .write_all(text.as_bytes())
                .and_then(|()| out.flush())
                .is_err()
            {
                return Ended::Failed;
            }
            Ended::Clean
        }
        Err(error) => {
            say!("silico schema: {error}");
            Ended::Failed
        }
    }
}

/// A command that rebuilds its files: `check` or `fmt`, with its option.
enum Rebuilding {
    Check { json: bool },
    Fmt { check: bool },
}

/// `files`, each run through `command` in turn, on one kernel pool, started once, whatever
/// directories they're in (each directory's cache beside its files); a pool that couldn't
/// be opened fails each rebuild that needs it, so a document's own problems the gate finds
/// are still said, needing no kernel.
fn rebuilding(command: &Rebuilding, files: &[PathBuf]) -> Ended {
    each(&Sessions::new(KernelPool::start()), command, files)
}

/// `command` of each of `files`, on `sessions`: the worst way any ended.
fn each<P: Clients + 'static>(
    sessions: &Sessions<P>,
    command: &Rebuilding,
    files: &[PathBuf],
) -> Ended {
    files
        .iter()
        .map(|file| match *command {
            Rebuilding::Check { json } => check(sessions, file, json),
            Rebuilding::Fmt { check } => fmt(sessions, file, check),
        })
        .fold(Ended::Clean, Ended::and)
}

/// `file` opened as a session of `sessions` (`open`), `then` run on it, and the session
/// closed (`close`): how `then` ended; or, if the file couldn't be opened, how `unopened`,
/// given why, ended.
fn in_session<P: Clients + 'static>(
    sessions: &Sessions<P>,
    file: &Path,
    unopened: impl FnOnce(Problems) -> Ended,
    then: impl FnOnce(&SessionId) -> Ended,
) -> Ended {
    let opened = sessions.open(file, false).wait();
    let id = match (opened.result, opened.session) {
        (Ok(_), Some(session)) => session.id,
        (Err(problems), _) => return unopened(problems),
        (Ok(_), None) => {
            return unopened(Problems::from(
                Error::new(ErrorCode::Internal, "the file opened, but in no session")
                    .with_suggestion(silico_core::worker::SILICO_BUG),
            ));
        }
    };
    let ended = then(&id);
    match sessions.close(&id).wait().result {
        Ok(_) => ended,
        Err(problems) => ended.and(refused(file, &problems)),
    }
}

/// Whether `error`, a command's refusal, says it couldn't run, not that the document has
/// problems: a file that couldn't be read or written, or changed while it was rebuilt, a
/// command cancelled, or a kernel fault (docs/cli.md's exit 2).
fn couldnt_run(error: &Error) -> bool {
    let code = error.code();
    code.is_kernel_fault()
        || matches!(
            code,
            ErrorCode::IoFailed | ErrorCode::Conflict | ErrorCode::Cancelled
        )
}

/// `error`, a refusal of a command on `file`, as the CLI says it: the request's `path`, which
/// the command line fills with the file each line already names, not named again.
fn of_file(error: &Error) -> Error {
    if error.parameter() == Some("path") {
        error.clone().without_parameter()
    } else {
        error.clone()
    }
}

/// A command's refusal of `file`, each reason on stderr: exit 2 if any says it couldn't
/// run ([`couldnt_run`]), otherwise 1, the document's (or the file's) own problems.
fn refused(file: &Path, problems: &Problems) -> Ended {
    print_problems(file, problems, of_file);
    refusal_ended(problems)
}

fn refusal_ended(problems: &Problems) -> Ended {
    if problems.errors().iter().any(couldnt_run) {
        Ended::Failed
    } else {
        Ended::Problems
    }
}

/// `silico check FILE`: open, the session's `check` of its current revision, close. Its
/// problems (failed features and sketches, and drift of fingerprints and of sketches' stored
/// solves), each on stderr, and a kernel fault the rebuild met after them; or, with
/// `--json`, one line of JSON on stdout for each file: `{"file", "ok", "problems", "total"}`,
/// each problem an error as the core's error contract has it, or, for a file that couldn't
/// be checked (a refusal that says it couldn't run, or a kernel fault), `{"file", "ok":
/// false, "error"}`, the error that stopped it, in the same shape
/// (tests/contract/check-line.json).
fn check<P: Clients + 'static>(sessions: &Sessions<P>, file: &Path, json: bool) -> Ended {
    let refusal = |problems: Problems| {
        if !json {
            return refused(file, &problems);
        }
        match problems.errors().iter().find(|e| couldnt_run(e)) {
            Some(error) => failed_line(file, &of_file(error)),
            None => {
                let errors: Vec<Error> = problems.errors().iter().map(of_file).collect();
                problems_line(file, &errors, problems.total());
                Ended::Problems
            }
        }
    };
    in_session(sessions, file, refusal, |id| {
        let Response { model, result, .. } = sessions.check(id, ModelChoice::Current).wait();
        let answer = match result {
            Ok(answer) => answer,
            Err(problems) => return refusal(problems),
        };
        let problems = answer.problems;
        // A kernel fault isn't the document's: given beside its problems, and the check's
        // failure (exit 2), as a file that couldn't be checked.
        if let Some(fault) = model.and_then(|read| read.fault) {
            if json {
                return failed_line(file, &fault);
            }
            print_problems(file, &problems, Error::clone);
            say!("{}: {fault}", shown(file));
            return Ended::Failed;
        }
        if json {
            problems_line(file, problems.errors(), problems.total());
        } else {
            print_problems(file, &problems, Error::clone);
        }
        if problems.is_empty() {
            Ended::Clean
        } else {
            Ended::Problems
        }
    })
}

/// `check --json`'s line for a file it checked: `{"file", "ok", "problems", "total"}`.
fn problems_line(file: &Path, errors: &[Error], total: u64) {
    line(serde_json::json!({
        "file": named(file),
        "ok": total == 0,
        "problems": errors,
        "total": total,
    }));
}

/// `check --json`'s line for a file it couldn't check: `{"file", "ok": false, "error"}`.
fn failed_line(file: &Path, error: &Error) -> Ended {
    line(serde_json::json!({"file": named(file), "ok": false, "error": error}));
    Ended::Failed
}

/// `silico fmt FILE`: open, the session's `format` with no features named (every drift
/// accepted, decision 31) and `write` unless `--check`, close. The session writes the file
/// (the one it names, through a symbolic link) in canonical form with its fingerprints as
/// rebuilt and its sketches as solved, through `file::save`; this says on stderr the
/// problems it couldn't fix (a feature that didn't build keeps the fingerprint stored with
/// it, and a sketch that didn't solve keeps what its file holds, and the file is written),
/// then which fingerprints and sketches' solves it wrote; with `--check`, whether it would
/// write. A read-only file, or one changed while it was rebuilt, isn't written (exit 2). A
/// kernel fault the rebuild met is said last (exit 2): the features it stopped keep what's
/// stored with them, as failed ones do, and the rest is written (decision 32).
fn fmt<P: Clients + 'static>(sessions: &Sessions<P>, file: &Path, check_only: bool) -> Ended {
    in_session(
        sessions,
        file,
        |problems| refused(file, &problems),
        |id| {
            let Response { model, result, .. } = sessions.format(id, None, !check_only).wait();
            let fault = model.and_then(|read| read.fault);
            let mut ended = match result {
                Err(problems) => refused(file, &problems),
                Ok(formatted) => {
                    print_problems(file, &formatted.problems, Error::clone);
                    let mut ended = if formatted.problems.is_empty() {
                        Ended::Clean
                    } else {
                        Ended::Problems
                    };
                    if check_only {
                        if formatted.changed {
                            say!(
                                "{}: not in canonical form; `silico fmt` would rewrite it",
                                shown(file)
                            );
                            ended = ended.and(Ended::Problems);
                        }
                    } else {
                        // A feature whose drift it accepted had a fingerprint stored, which it
                        // rewrote; every other had none.
                        for id in &formatted.refingerprinted {
                            let what = if formatted.accepted.contains(id) {
                                "rewrote"
                            } else {
                                "wrote"
                            };
                            say!("{}: {what} the fingerprint of {id}", shown(file));
                        }
                        for id in &formatted.resolved {
                            say!("{}: wrote the solve of sketch {id}", shown(file));
                        }
                    }
                    ended
                }
            };
            if let Some(fault) = fault {
                say!("{}: {fault}", shown(file));
                ended = Ended::Failed;
            }
            ended
        },
    )
}

/// `silico show FILE`: open, the session's `outline`, close; the outline on stdout. Needs no
/// kernel: it's the document as written, for reading and for `git diff`.
fn show(file: &Path) -> Ended {
    // A kernel and a cache it never uses: nothing is rebuilt, and the user's cache secret is
    // neither read nor made.
    let sessions = Sessions::with_cache_opener(
        KernelPool::refusing(
            Error::new(ErrorCode::Internal, "`silico show` starts no kernel")
                .with_suggestion(silico_core::worker::SILICO_BUG),
        ),
        |dir| Cache::unsealed(dir),
    );
    in_session(
        &sessions,
        file,
        |problems| refused(file, &problems),
        |id| match sessions.outline(id).wait().result {
            Ok(answer) => {
                let mut out = std::io::stdout().lock();
                if out
                    .write_all(answer.outline.as_bytes())
                    .and_then(|()| out.flush())
                    .is_err()
                {
                    return Ended::Failed;
                }
                Ended::Clean
            }
            Err(problems) => refused(file, &problems),
        },
    )
}

/// `silico textconv FILE`, git's text conversion for `*.silico` (docs/cli.md): the outline,
/// as `show` prints it, if the file reads as a document; if not (a merge's conflict
/// markers, an edit half made), the file as it is, so `git diff` still shows it. Fails only
/// if the file can't be read or stdout written.
///
/// Not a session's: the file as it is must be passed through whatever it holds, and a
/// session holds neither a file longer than a document may be nor one whose name isn't
/// UTF-8 (`open` refuses both), so this reads the file once, itself, and outlines it with
/// the core's `command::outline`, which a session's `outline` gives too.
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
        Err(e) => fail(
            Error::new(ErrorCode::IoFailed, format!("couldn't be converted: {e}"))
                .with_suggestion("check that the output can be written (a closed pipe stops it)"),
        ),
    }
}

/// A file's name (its bytes, any that aren't UTF-8 replaced), as a JSON line holds it: the
/// name itself, which JSON escapes only as far as its own controls go (U+0000 to U+001F, a
/// quote, a backslash); a program reads it, and a person reading the line in a terminal
/// sees the rest as it is.
fn named(file: &Path) -> String {
    file.display().to_string()
}

/// A file's name as a terminal is shown it: as it is, or, if it holds a character a
/// terminal would act on or draw as nothing (an escape sequence in a cloned repository's
/// file name), quoted with those escaped, as an outline quotes a document's text.
fn shown(file: &Path) -> String {
    let name = named(file);
    let quoted = silico_core::command::quoted(&name);
    if quoted.len() == name.len() + 2 {
        name
    } else {
        quoted
    }
}

fn line(value: serde_json::Value) {
    let _ = writeln!(std::io::stdout().lock(), "{value}");
}

/// Each problem on stderr, after the file it's about, as `said` gives it: its code and
/// words, then the feature, elements, and parameter at fault, and what to do (the error's
/// own `Display`); and how many more there are, if not all are listed.
fn print_problems(file: &Path, problems: &Problems, said: impl Fn(&Error) -> Error) {
    for error in problems.errors() {
        say!("{}: {}", shown(file), said(error));
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
