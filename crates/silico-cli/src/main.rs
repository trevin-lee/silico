//! The `silico` command-line client (docs/cli.md): `check`, `fmt`, `show`, `cache clear`,
//! `export`, and `schema`, each a thin client of the core's commands (`silico_core::command`),
//! which MCP and the app run too; `textconv`, for `git diff`; and `serve`, the MCP server on
//! stdin and stdout (`silico_mcp`; docs/design/m4.9-mcp.md). `check`, `fmt`, `show`,
//! `cache clear`, and `export` each open a file as a session, run one session command on it
//! (`check`, `format`, `outline`, `cache_clear`, `export`), and close it
//! (docs/design/m4.1-command-api.md, section 9); `fmt --merge-touching` runs `check` and a
//! `sketch_merge` of each cluster of touching ends before its `format`, in one transaction
//! (`begin`, then `commit`). `check` and `fmt` run every file on one
//! kernel pool and one sketch solver, and `export` its file on one of each; `show` and
//! `cache clear` rebuild nothing, so they start neither.
//!
//! Exit codes: 0, nothing wrong; 1, a document has problems (or, with `fmt --check`, isn't
//! in canonical form, or a file is longer than a document may be); 2, the command couldn't
//! run (a usage error, a file that couldn't be read or written, or whose name isn't UTF-8,
//! a file that changed while `fmt` rebuilt it, or a kernel or sketch solver that couldn't
//! start or failed in a way that isn't the document's: a kernel fault, said after the
//! document's problems).

use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;
use silico_core::cache::Cache;
use silico_core::command::{self, CheckModel, Problems, Response, SessionId, Sessions, Warnings};
use silico_core::file::opened_and_read;
use silico_core::pool::KernelPool;
use silico_core::{Error, ErrorCode, Said};

/// A line on stderr (the workspace forbids `eprintln!`, which panics if stderr is closed).
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr().lock(), $($arg)*);
    }};
}

const USAGE: &str = "usage: silico check [--json] [--fresh] [--] FILE...
       silico fmt [--check] [--merge-touching] [--] FILE...
       silico show [--] FILE
       silico cache clear [--] FILE...
       silico export [--tolerance=MM] [--features=ID,...] [--replace] [--] FILE OUT.3mf
       silico export [--features=ID,...] [--replace] [--] FILE OUT.step|OUT.stp
       silico textconv [--] FILE
       silico schema [NAME]
       silico serve [--workspace DIR]";

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

    /// How a command whose result is `problems` ended: clean, or with the document's
    /// problems.
    fn of(problems: &Problems) -> Ended {
        if problems.is_empty() {
            Ended::Clean
        } else {
            Ended::Problems
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
    Check {
        json: bool,
        fresh: bool,
    },
    Fmt {
        check: bool,
        merge_touching: bool,
    },
    Show,
    CacheClear,
    Export {
        tolerance: Option<String>,
        features: Option<String>,
        replace: bool,
    },
    Textconv,
    Schema,
    Serve,
}

impl Command {
    /// The command `name` with `flags`; or, for a command or an option there's no such
    /// thing as, what to say. `cache`'s subcommand is its first word after it, `clear`, taken
    /// from `files` ([`Command::parse_cache`]).
    fn parse(name: &str, flags: &[String]) -> Result<Command, String> {
        let flag = |option: &str| flags.iter().any(|f| f == option);
        // An option's value, `--name=VALUE`, the last given if it's given more than once.
        let value = |option: &str| {
            flags
                .iter()
                .rev()
                .find_map(|f| f.strip_prefix(option)?.strip_prefix('='))
                .map(str::to_owned)
        };
        let (command, options): (Command, &[&str]) = match name {
            "check" => (
                Command::Check {
                    json: flag("--json"),
                    fresh: flag("--fresh"),
                },
                &["--json", "--fresh"],
            ),
            "cache" => (Command::CacheClear, &[]),
            "fmt" => (
                Command::Fmt {
                    check: flag("--check"),
                    merge_touching: flag("--merge-touching"),
                },
                &["--check", "--merge-touching"],
            ),
            "show" => (Command::Show, &[]),
            "export" => (
                Command::Export {
                    tolerance: value("--tolerance"),
                    features: value("--features"),
                    replace: flag("--replace"),
                },
                &["--tolerance=", "--features=", "--replace"],
            ),
            "textconv" => (Command::Textconv, &[]),
            "schema" => (Command::Schema, &[]),
            "serve" => (Command::Serve, &[WORKSPACE]),
            _ => return Err(format!("silico: no command {name:?}")),
        };
        // An option takes a value if it ends in `=`; every other is a flag, named exactly.
        let known = |f: &str| {
            options.iter().any(|o| match o.strip_suffix('=') {
                Some(_) => f.starts_with(o),
                None => f == *o,
            })
        };
        match flags.iter().find(|f| !known(f)) {
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
    // Options, then files; `--` ends the options, so a file may be named like one. An
    // option's value (`--workspace DIR`, or `--workspace=DIR`) is taken as the OS gives it.
    let mut flags: Vec<String> = Vec::new();
    let mut workspaces: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut options = true;
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        if options && arg == "--" {
            options = false;
        } else if options && arg == WORKSPACE {
            let Some(dir) = args.next() else {
                say!("silico: {WORKSPACE} needs a directory\n{USAGE}");
                return Ended::Failed.code();
            };
            flags.push(WORKSPACE.to_owned());
            workspaces.push(PathBuf::from(dir));
        } else if options && let Some(dir) = valued(arg, WORKSPACE) {
            flags.push(WORKSPACE.to_owned());
            workspaces.push(PathBuf::from(dir));
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
    // `cache` takes its subcommand, `clear`, before its files (and before `--`, which ends
    // the options, not the words).
    if matches!(command, Command::CacheClear) {
        match subcommand(&mut files) {
            Ok(()) => {}
            Err(why) => {
                say!("{why}\n{USAGE}");
                return Ended::Failed.code();
            }
        }
    }
    // `check` and `fmt` run every file on one kernel pool, started once, whatever
    // directories they're in (each directory's cache beside its files), and one sketch
    // solver. A pool that couldn't be started fails each rebuild that needs it, so the
    // problems the gate finds are still said, needing no kernel.
    let each = |run: &dyn Fn(&Sessions<KernelPool>, &Path) -> Ended| {
        let sessions = Sessions::new(KernelPool::start());
        files
            .iter()
            .map(|file| run(&sessions, file))
            .fold(Ended::Clean, Ended::and)
    };
    let ended = match (command, files.as_slice()) {
        (Command::Serve, []) => match workspaces.as_slice() {
            [] => serve(None),
            [dir] => serve(Some(dir)),
            [_, _, ..] => {
                say!("silico serve: {WORKSPACE} is given once\n{USAGE}");
                Ended::Failed
            }
        },
        (Command::Schema, []) => schema("document"),
        (Command::Schema, [name]) => schema(&name.to_string_lossy()),
        (Command::Export { .. }, [] | [_] | [_, _, _, ..])
        | (_, [])
        | (Command::Show | Command::Textconv | Command::Schema, [_, _, ..])
        | (Command::Serve, [_, ..]) => {
            say!("{USAGE}");
            Ended::Failed
        }
        (
            Command::Export {
                tolerance,
                features,
                replace,
            },
            [file, out],
        ) => export(
            &Sessions::new(KernelPool::start()),
            file,
            out,
            tolerance.as_deref(),
            features.as_deref(),
            replace,
        ),
        (Command::Check { json, fresh }, _) => {
            each(&|sessions, file| check(sessions, file, json, fresh))
        }
        (
            Command::Fmt {
                check,
                merge_touching,
            },
            _,
        ) => each(&|sessions, file| fmt(sessions, file, check, merge_touching)),
        (Command::CacheClear, _) => {
            // Nothing is rebuilt: no kernel is started, and the user's cache secret is
            // neither read nor made (a clear removes entries whoever sealed them).
            let sessions =
                Sessions::with_cache_opener(no_kernel("cache clear"), |dir| Cache::unsealed(dir));
            files
                .iter()
                .map(|file| cache_clear(&sessions, file))
                .fold(Ended::Clean, Ended::and)
        }
        (Command::Show, [file]) => show(file),
        (Command::Textconv, [file]) => textconv(file),
    };
    ended.code()
}

/// `serve`'s option naming its workspace.
const WORKSPACE: &str = "--workspace";

/// The value of `arg` if it's the option `name` with its value joined (`--workspace=DIR`):
/// the value as the OS gives it, whatever its bytes.
fn valued<'a>(arg: &'a OsStr, name: &str) -> Option<&'a OsStr> {
    arg.as_bytes()
        .strip_prefix(name.as_bytes())?
        .strip_prefix(b"=")
        .map(OsStr::from_bytes)
}

/// How often `serve` looks whether SIGTERM came.
const SIGNAL_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// `silico serve [--workspace DIR]`: the MCP server on stdin and stdout, until stdin ends,
/// stdout closes, or SIGTERM, its sessions in the workspace `DIR` (or the current
/// directory), on one kernel pool, sketch solver, and render worker. `/` (or a drive's
/// root) is refused: some MCP clients start servers there, which would make the whole disk
/// the workspace. It starts with all four of its workers (kernel, sketch, render, STEP) or
/// not at all: any that isn't there (or can't be read) is said, each in one message on
/// stderr naming where it looked, and it exits 2 before the handshake (one there that can't
/// be run is met when it's first asked, as its fault). Logs go to stderr; stdout holds
/// nothing but MCP's messages. A second
/// SIGTERM, while the first is still ending it, ends it at once (exit 1).
fn serve(workspace: Option<&Path>) -> Ended {
    let root = match workspace.map_or_else(std::env::current_dir, |dir| Ok(dir.to_owned())) {
        Ok(root) => root,
        Err(e) => {
            say!("silico serve: the current directory can't be read: {e}");
            return Ended::Failed;
        }
    };
    let root = match std::fs::canonicalize(&root) {
        Ok(root) => root,
        Err(e) => {
            say!(
                "silico serve: the workspace {} can't be reached: {e}",
                root.display()
            );
            return Ended::Failed;
        }
    };
    if root.parent().is_none() {
        say!(
            "silico serve: the workspace can't be {}, the root of a disk: give the directory the documents are in (--workspace DIR, or start silico serve in it)",
            root.display()
        );
        return Ended::Failed;
    }
    // The workers, found before the handshake, all four or none: a missing one is a broken
    // install, said here, every one of them at once, rather than met by the agent at the
    // command that first needs it.
    let pool = KernelPool::start();
    let mut missing = Vec::new();
    if let Some(why) = pool.refused() {
        missing.push(format!(
            "silico serve: there's no kernel worker it can use, so nothing can be built: {why}"
        ));
    }
    if let Err(e) = silico_core::sketch::worker_path() {
        missing.push(format!(
            "silico serve: there's no sketch worker, so no sketch can be solved: {e}"
        ));
    }
    if let Err(e) = silico_core::step_worker::worker_path() {
        missing.push(format!(
            "silico serve: there's no STEP worker, so nothing can be exported as STEP: {e}"
        ));
    }
    let renderer = silico_core::render::worker_path().and_then(|exe| {
        silico_core::render::RenderClient::start(
            &exe,
            silico_core::render::RenderSettings::default(),
        )
    });
    let renderer = match renderer {
        Ok(renderer) => Some(renderer),
        Err(e) => {
            missing.push(format!(
                "silico serve: there's no render worker, so nothing can be rendered: {e}"
            ));
            None
        }
    };
    let (Some(renderer), true) = (renderer, missing.is_empty()) else {
        say!("{}", missing.join("\n"));
        return Ended::Failed;
    };
    let sessions = Sessions::new(pool).rendering(renderer);
    let sessions = match sessions.in_workspace(&root) {
        Ok(sessions) => sessions,
        Err(e) => {
            say!("silico serve: {e}");
            return Ended::Failed;
        }
    };
    let server = match silico_mcp::Server::new(sessions) {
        Ok(server) => server,
        Err(e) => {
            say!("silico serve: {e}");
            return Ended::Failed;
        }
    };
    let stop = server.stopper();
    let term = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The second first: a SIGTERM that finds the flag set (the first came) exits at once.
    if let Err(e) = signal_hook::flag::register_conditional_shutdown(
        signal_hook::consts::SIGTERM,
        1,
        std::sync::Arc::clone(&term),
    ) {
        tracing::warn!("a second SIGTERM can't be made to end the server at once: {e}");
    }
    if let Err(e) =
        signal_hook::flag::register(signal_hook::consts::SIGTERM, std::sync::Arc::clone(&term))
    {
        tracing::warn!("SIGTERM can't be caught, so it ends the server at once: {e}");
    }
    let watching = std::thread::Builder::new()
        .name("sigterm".to_owned())
        .spawn(move || {
            while !term.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(SIGNAL_POLL);
            }
            tracing::info!("SIGTERM: stopping");
            stop.stop();
        });
    if let Err(e) = watching {
        tracing::warn!("SIGTERM can't be watched for: {e}");
    }
    tracing::info!(workspace = %root.display(), "serving MCP on stdin and stdout");
    match server.serve(std::io::BufReader::new(std::io::stdin()), std::io::stdout()) {
        silico_mcp::Ended::InputFailed(e) => {
            say!("silico serve: stdin couldn't be read: {e}");
            Ended::Failed
        }
        silico_mcp::Ended::ThreadFailed(e) => {
            say!("silico serve: {e}");
            Ended::Failed
        }
        ended => {
            tracing::info!(?ended, "the client went away");
            Ended::Clean
        }
    }
}

/// `cache`'s subcommand, taken from the front of `files`: `clear`, the only one; or what to
/// say of another, or of none.
fn subcommand(files: &mut Vec<PathBuf>) -> Result<(), String> {
    match files
        .first()
        .map(|word| word.to_string_lossy().into_owned())
    {
        Some(word) if word == "clear" => {
            files.remove(0);
            Ok(())
        }
        Some(word) => Err(format!("silico cache: no subcommand {word:?}")),
        None => Err("silico cache: which? clear".to_owned()),
    }
}

/// A kernel for a command that rebuilds nothing, `silico command`: starting none, and
/// refusing, as a bug, anything asked of it.
fn no_kernel(command: &str) -> KernelPool {
    KernelPool::refusing(
        Error::new(
            ErrorCode::Internal,
            format!("`silico {command}` starts no kernel"),
        )
        .with_suggestion(silico_core::worker::SILICO_BUG),
    )
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

/// `file` opened as a session of `sessions` (`open`), `then` run on it, and the session
/// closed (`close`): how `then` ended; or, if the file couldn't be opened, how `unopened`,
/// given open's refusal as the CLI says it, ended. `command` is the `silico` command run,
/// for a refusal's advice.
fn in_session(
    sessions: &Sessions<KernelPool>,
    file: &Path,
    command: &str,
    unopened: impl FnOnce(Refusal) -> Ended,
    then: impl FnOnce(&SessionId) -> Ended,
) -> Ended {
    let id = match sessions.open(file, false).wait().result {
        Ok(opened) => opened.session,
        Err(problems) => return unopened(Refusal::of_open(file, &problems)),
    };
    let ended = then(&id);
    match sessions.close(&id).wait().result {
        Ok(_) => ended,
        Err(problems) => ended.and(Refusal::of_command(command, &problems).said(file)),
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

/// A command's refusal of a file, as the CLI says it: each reason, the first listed and
/// all counted; a kernel fault the rebuild met, if the refusal carries one, said after
/// them; and whether it says the command couldn't run (exit 2), not that the document, or
/// the file, has problems (exit 1).
struct Refusal {
    errors: Vec<Error>,
    total: u64,
    fault: Option<Error>,
    failed: bool,
}

impl Refusal {
    /// `open`'s refusal of `file`. Its `path` is the file each line already names, so isn't
    /// named again; but a name that isn't UTF-8 is a file that couldn't be had, as one
    /// unreadable is (exit 2), its `path` the parameter at fault.
    fn of_open(file: &Path, problems: &Problems) -> Self {
        let unnamed = file.to_str().is_none();
        let errors: Vec<Error> = problems
            .errors()
            .iter()
            .map(|error| {
                if !unnamed && error.parameter() == Some("path") {
                    error.clone().without_parameter()
                } else {
                    error.clone()
                }
            })
            .collect();
        Self::of(errors, problems.total(), unnamed)
    }

    /// The refusal of a command, `silico command`'s, on a file its session opened: as the
    /// session gives it, but how to go on about the file (a save that found it changed, a
    /// session that couldn't take it as it is now) said as the CLI does, which opens a
    /// session for each run, not as a session's client goes on. A file it couldn't read
    /// again is to be checked and the command run again; one grown longer than a document
    /// may be is to be checked as open's refusal of it says. (A file another session holds
    /// can't be one: the CLI holds one session at a time.)
    fn of_command(command: &str, problems: &Problems) -> Self {
        let errors = problems
            .errors()
            .iter()
            .map(|error| {
                let about_the_file = error.parameter() == Some("path");
                if error.suggestion() == Some(silico_core::file::REPLACE_ADVICE) {
                    error
                        .clone()
                        .with_suggestion("give --replace to replace it, or another path")
                } else if about_the_file && error.code() == ErrorCode::Conflict {
                    error.clone().with_suggestion(format!(
                        "nothing was written: run `silico {command}` again, on the file as it \
                         is now"
                    ))
                } else if about_the_file && error.cause() == Some(command::NOT_RELOADED) {
                    let suggestion = match error.code() {
                        ErrorCode::IoFailed => "check the file is there and run it again",
                        _ => command::TOO_LONG,
                    };
                    error.clone().with_suggestion(suggestion)
                } else {
                    error.clone()
                }
            })
            .collect();
        Self::of(errors, problems.total(), false)
    }

    fn of(errors: Vec<Error>, total: u64, failed: bool) -> Self {
        let failed = failed || errors.iter().any(couldnt_run);
        Refusal {
            errors,
            total,
            fault: None,
            failed,
        }
    }

    /// This refusal of `silico command`, with `fault`, a kernel fault its response's model
    /// read carries: the command's failure (exit 2), as a fault beside a result is, worded
    /// with running the command again ([`with_retry`]). Said once: where the refusal lists
    /// it already (a solid it stopped, asked for), there, reworded from the fault as met, not
    /// as a session worded it.
    fn with_fault(self, fault: Option<Error>, command: &str) -> Self {
        let Some(fault) = fault else {
            return self;
        };
        let same =
            |error: &Error| error.code() == fault.code() && error.message() == fault.message();
        if self.errors.iter().any(same) {
            let errors = self
                .errors
                .iter()
                .map(|error| {
                    if same(error) {
                        with_retry(&fault, command)
                    } else {
                        error.clone()
                    }
                })
                .collect();
            return Refusal {
                errors,
                failed: true,
                ..self
            };
        }
        Refusal {
            fault: Some(with_retry(&fault, command)),
            failed: true,
            ..self
        }
    }

    fn ended(&self) -> Ended {
        if self.failed {
            Ended::Failed
        } else {
            Ended::Problems
        }
    }

    /// Each reason on stderr, after `file`, then the kernel fault, if any.
    fn said(&self, file: &Path) -> Ended {
        print_problems(file, &self.errors, self.total);
        if let Some(fault) = &self.fault {
            say!("{}: {fault}", shown(file));
        }
        self.ended()
    }

    /// The error that stopped the command, if it couldn't run: a reason that says so, or
    /// else the kernel fault, or else the first reason.
    fn stopped(&self) -> Option<&Error> {
        self.errors
            .iter()
            .find(|error| couldnt_run(error))
            .or(self.fault.as_ref())
            .or_else(|| self.errors.first().filter(|_| self.failed))
    }

    /// `check --json`'s line for `file`: the reason it couldn't run, or the problems.
    fn line(&self, file: &Path) -> Ended {
        match self.stopped() {
            Some(error) => failed_line(file, error),
            None => {
                problems_line(file, &self.errors, self.total);
                Ended::Problems
            }
        }
    }
}

/// `fault`, a kernel fault the rebuild of a file met (its suggestion the worker's own), as
/// `silico command` says it: worded as every client words one (`command::fault_suggestion`),
/// trying again by running the command again, which rebuilds the file anew; said once, and
/// not at all after a crash the request's values caused, which the same file would cause
/// again.
fn with_retry(fault: &Error, command: &str) -> Error {
    let suggestion = command::fault_suggestion(fault, command::TryAgainBy::Running(command));
    fault.clone().with_suggestion(suggestion)
}

/// `silico check FILE`: open, the session's `check` of its current revision, close. Its
/// problems (failed features and sketches, and drift of fingerprints and of sketches' stored
/// solves), each on stderr, then its warnings, each marked `warning:` (they never change
/// the exit code), and a kernel fault the rebuild met after them; or, with
/// `--json`, its warnings on stderr as without it, and one line of JSON on stdout for each
/// file: `{"file", "ok", "problems", "total"}`,
/// each problem an error as the core's error contract has it, or, for a file that couldn't
/// be checked (a refusal that says it couldn't run, or a kernel fault), `{"file", "ok":
/// false, "error"}`, the error that stopped it, in the same shape
/// (tests/contract/check-line.json).
fn check(sessions: &Sessions<KernelPool>, file: &Path, json: bool, fresh: bool) -> Ended {
    let refused = |refusal: Refusal| {
        if json {
            refusal.line(file)
        } else {
            refusal.said(file)
        }
    };
    in_session(sessions, file, "check", refused, |id| {
        let Response { model, result, .. } =
            sessions.check(id, CheckModel::Current { fresh }).wait();
        let (problems, warnings) = match result {
            Ok(answer) => (answer.problems, answer.warnings),
            Err(problems) => {
                let fault = model.and_then(|read| read.fault);
                return refused(Refusal::of_command("check", &problems).with_fault(fault, "check"));
            }
        };
        // A kernel fault isn't the document's: given after its problems, and the check's
        // failure (exit 2), as a file that couldn't be checked.
        if let Some(fault) = model.and_then(|read| read.fault) {
            let fault = with_retry(&fault, "check");
            if json {
                print_warnings(file, &warnings);
                return failed_line(file, &fault);
            }
            print_problems(file, problems.errors(), problems.total());
            print_warnings(file, &warnings);
            say!("{}: {fault}", shown(file));
            return Ended::Failed;
        }
        if json {
            // The line is a contract with no warnings in it: they're said on stderr, as
            // without `--json`.
            print_warnings(file, &warnings);
            problems_line(file, problems.errors(), problems.total());
        } else {
            print_problems(file, problems.errors(), problems.total());
            print_warnings(file, &warnings);
        }
        // Warnings fail nothing: the exit code is the problems' alone.
        Ended::of(&problems)
    })
}

/// `check --json`'s line for a file it checked: `{"file", "ok", "problems", "total"}`.
fn problems_line(file: &Path, errors: &[Error], total: u64) {
    line(&ProblemsLine::new(file, errors, total));
}

/// `check --json`'s line for a file it couldn't check: `{"file", "ok": false, "error"}`.
fn failed_line(file: &Path, error: &Error) -> Ended {
    line(&FailedLine::new(file, error));
    Ended::Failed
}

/// `check --json`'s line for a file it checked, its keys sorted, as the contract
/// (`tests/contract/check-line.json`) holds them. A struct, not a `serde_json::Value`: a
/// value's keys come out in the order they were added, not sorted, in a build where any
/// crate turns on serde_json's `preserve_order`.
#[derive(Serialize)]
struct ProblemsLine<'a> {
    file: String,
    ok: bool,
    problems: Vec<LineError<'a>>,
    total: u64,
}

impl<'a> ProblemsLine<'a> {
    fn new(file: &Path, errors: &'a [Error], total: u64) -> Self {
        ProblemsLine {
            file: named(file),
            ok: total == 0,
            problems: errors.iter().map(LineError::new).collect(),
            total,
        }
    }
}

/// `check --json`'s line for a file it couldn't check, its keys sorted.
#[derive(Serialize)]
struct FailedLine<'a> {
    error: LineError<'a>,
    file: String,
    ok: bool,
}

impl<'a> FailedLine<'a> {
    fn new(file: &Path, error: &'a Error) -> Self {
        FailedLine {
            error: LineError::new(error),
            file: named(file),
            ok: false,
        }
    }
}

/// An error in the core's error contract, as a `check --json` line holds it: every field
/// the error writes, its keys sorted.
#[derive(Serialize)]
struct LineError<'a> {
    candidates: &'a [String],
    cause: Option<&'a str>,
    code: &'static str,
    elements: &'a [String],
    elements_total: u64,
    feature: Option<&'a str>,
    message: &'a str,
    parameter: Option<&'a str>,
    suggestion: Option<&'a str>,
}

impl<'a> LineError<'a> {
    /// `error`'s every field, taken whole ([`Said`]), so a field an error gains is one this
    /// must write.
    fn new(error: &'a Error) -> Self {
        let Said {
            code,
            message,
            feature,
            elements,
            elements_total,
            parameter,
            cause,
            suggestion,
            candidates,
        } = error.said();
        LineError {
            candidates,
            cause,
            code,
            elements,
            elements_total,
            feature,
            message,
            parameter,
            suggestion,
        }
    }
}

/// `silico fmt FILE`: open, the session's `format` with no features named (every drift
/// accepted, decision 31) and `write` unless `--check`, close. The session writes the file
/// (the one it names, through a symbolic link) in canonical form with its fingerprints as
/// rebuilt and its sketches as solved, through `file::save_over`; this says on stderr the
/// problems it couldn't fix (a feature that didn't build keeps the fingerprint stored with
/// it, and a sketch that didn't solve keeps what its file holds, and the file is written),
/// then which fingerprints and sketches' solves it wrote; with `--check`, whether it would
/// write. A read-only file, or one changed while it was rebuilt, isn't written (exit 2). A
/// kernel fault the rebuild met is said last (exit 2): the features it stopped keep what's
/// stored with them, as failed ones do, and the rest is written (decision 32).
///
/// With `--merge-touching` (docs/design/m5.0-app.md, `sketch_merge`, decision 17), first,
/// inside one transaction, one `sketch_merge` for each `ends_not_joined` cluster `check`
/// warns of, keeping the cluster's least ID ([`merge_touching`]); then the `format`, held in
/// the transaction, and its `commit`, which writes the file (with `--check`, a `rollback`,
/// nothing written, and a merge it would make said as a rewrite it would make).
fn fmt(
    sessions: &Sessions<KernelPool>,
    file: &Path,
    check_only: bool,
    merge_touching: bool,
) -> Ended {
    in_session(
        sessions,
        file,
        "fmt",
        |refusal| refusal.said(file),
        |id| {
            let mut ended = Ended::Clean;
            let mut merged = false;
            if merge_touching {
                if let Err(problems) = sessions.begin(id).wait().result {
                    return Refusal::of_command("fmt", &problems).said(file);
                }
                let (made, end) = self::merge_touching(sessions, id, file, check_only);
                merged = made;
                ended = ended.and(end);
                if matches!(ended, Ended::Failed) {
                    let _ = sessions.rollback(id).wait();
                    return ended;
                }
            }
            let Response { model, result, .. } = sessions.format(id, None, !check_only).wait();
            let formatted = match result {
                Ok(formatted) => formatted,
                Err(problems) => {
                    if merge_touching {
                        let _ = sessions.rollback(id).wait();
                    }
                    let fault = model.and_then(|read| read.fault);
                    return ended.and(
                        Refusal::of_command("fmt", &problems)
                            .with_fault(fault, "fmt")
                            .said(file),
                    );
                }
            };
            let problems = &formatted.problems;
            print_problems(file, problems.errors(), problems.total());
            ended = ended.and(Ended::of(problems));
            if check_only {
                if merge_touching {
                    let _ = sessions.rollback(id).wait();
                }
                if formatted.changed || merged {
                    say!(
                        "{}: not in canonical form; `silico fmt` would rewrite it",
                        shown(file)
                    );
                    ended = ended.and(Ended::Problems);
                }
            } else {
                // Inside the transaction, the format held what it would write, and the
                // commit writes it.
                if merge_touching {
                    match sessions.commit(id).wait().result {
                        Ok(_) => {}
                        Err(problems) => {
                            return ended.and(Refusal::of_command("fmt", &problems).said(file));
                        }
                    }
                }
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
            if let Some(fault) = model.and_then(|read| read.fault) {
                say!("{}: {}", shown(file), with_retry(&fault, "fmt"));
                ended = Ended::Failed;
            }
            ended
        },
    )
}

/// The most rounds of `check` and merges [`merge_touching`] makes: each round merges every
/// cluster `check` lists (the first hundred warnings, each the first hundred points), so a
/// cluster or a document with more needs another; a round that merges nothing new ends it.
const MERGE_ROUNDS: usize = 64;

/// `fmt --merge-touching`'s merges, in session `id`'s open transaction: for each
/// `ends_not_joined` cluster the current model's `check` warns of (by timeline, then by its
/// least point ID, as warnings are ordered), one `sketch_merge` of its points, the least ID
/// first, so the point it keeps is the same on every machine and the file written is too
/// (the merge makes no new ID). Each merge said on stderr (`merged` or, with `check_only`,
/// `would merge`); a cluster whose merge is refused is said with why, and the others still
/// merge (a refused merge leaves the transaction as it was), the document then having
/// problems (exit 1). Whether any merge was made, and how it ended: a kernel fault, or a
/// command that couldn't run, ends it (exit 2).
fn merge_touching(
    sessions: &Sessions<KernelPool>,
    id: &SessionId,
    file: &Path,
    check_only: bool,
) -> (bool, Ended) {
    use silico_core::WarningCode;
    use silico_core::document::{ElementId, FeatureId};
    let mut tried: std::collections::BTreeSet<(String, Vec<String>)> = Default::default();
    let mut merged = false;
    let mut ended = Ended::Clean;
    for _ in 0..MERGE_ROUNDS {
        let Response { model, result, .. } = sessions
            .check(id, CheckModel::Current { fresh: false })
            .wait();
        let warnings = match result {
            Ok(answer) => answer.warnings,
            Err(problems) => {
                let fault = model.and_then(|read| read.fault);
                return (
                    merged,
                    Refusal::of_command("fmt", &problems)
                        .with_fault(fault, "fmt")
                        .said(file),
                );
            }
        };
        if let Some(fault) = model.and_then(|read| read.fault) {
            say!("{}: {}", shown(file), with_retry(&fault, "fmt"));
            return (merged, Ended::Failed);
        }
        let clusters: Vec<(String, Vec<String>)> = warnings
            .listed()
            .iter()
            .filter(|w| w.code() == WarningCode::EndsNotJoined)
            .filter_map(|w| {
                let mut points = w.elements().to_vec();
                points.sort();
                Some((w.feature()?.to_owned(), points))
            })
            .filter(|cluster| !tried.contains(cluster))
            .collect();
        if clusters.is_empty() {
            break;
        }
        for (sketch, points) in clusters {
            tried.insert((sketch.clone(), points.clone()));
            let ids: Option<Vec<ElementId>> = points.iter().map(|p| ElementId::new(p)).collect();
            let (Some(feature), Some(ids)) = (FeatureId::new(&sketch), ids) else {
                continue;
            };
            let revision = match sessions.status(id).session {
                Some(info) => info.revision,
                None => return (merged, Ended::Failed),
            };
            let listed = points.join(", ");
            match sessions
                .sketch_merge(id, revision, feature, ids)
                .wait()
                .result
            {
                Ok(_) => {
                    merged = true;
                    let what = if check_only { "would merge" } else { "merged" };
                    say!(
                        "{}: {what} points {listed} of sketch {sketch}, keeping {}",
                        shown(file),
                        points[0]
                    );
                }
                Err(problems) => {
                    let refusal = Refusal::of_command("fmt", &problems);
                    if refusal.failed {
                        return (merged, refusal.said(file));
                    }
                    say!(
                        "{}: couldn't merge points {listed} of sketch {sketch}:",
                        shown(file)
                    );
                    ended = ended.and(refusal.said(file));
                }
            }
        }
    }
    (merged, ended)
}

/// `silico export FILE OUT.3mf` or `OUT.step`: open, the session's `export` of its current
/// revision (every body, or the features `--features` names; a 3MF meshed at `--tolerance`
/// mm, FreeCAD's 0.1 if none is given), close, the format the output's extension's. On
/// stdout, where the file went and each object in it: its feature and name, and for 3MF its
/// triangles, vertices, and volume; and the faces left out of it as bounding no material.
/// Refused as the command refuses (a path that's there without `--replace`, a document, a
/// tolerance out of range, a solid that isn't printable), each reason on stderr: nothing
/// written. An output that's neither, a tolerance with a STEP output, or a tolerance or a
/// feature's ID that isn't one is a usage error.
fn export(
    sessions: &Sessions<KernelPool>,
    file: &Path,
    out: &Path,
    tolerance: Option<&str>,
    features: Option<&str>,
    replace: bool,
) -> Ended {
    use silico_core::command::export::{DEFAULT_TOLERANCE, Export, ExportFormat};
    use silico_core::document::{FeatureId, Length};
    let is = |format: ExportFormat| {
        out.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            format
                .extensions()
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        })
    };
    let tolerance = match tolerance.map(Length::parse) {
        None => None,
        Some(Ok(tolerance)) => Some(tolerance),
        Some(Err(_)) => {
            say!("silico export: --tolerance takes a length in mm, such as 0.1\n{USAGE}");
            return Ended::Failed;
        }
    };
    let format = if is(ExportFormat::ThreeMf {
        tolerance: DEFAULT_TOLERANCE,
    }) {
        ExportFormat::ThreeMf {
            tolerance: tolerance.unwrap_or(DEFAULT_TOLERANCE),
        }
    } else if is(ExportFormat::Step) {
        if tolerance.is_some() {
            say!(
                "silico export: --tolerance is a 3MF mesh's; a STEP file holds each body's \
                 exact shape, and takes none\n{USAGE}"
            );
            return Ended::Failed;
        }
        ExportFormat::Step
    } else {
        say!("silico export: the output must end in .3mf, .step, or .stp, by its format\n{USAGE}");
        return Ended::Failed;
    };
    let features = match features {
        None => None,
        Some(list) => {
            let ids: Option<Vec<FeatureId>> = list.split(',').map(FeatureId::new).collect();
            match ids {
                Some(ids) => Some(ids),
                None => {
                    say!(
                        "silico export: --features takes features' IDs, separated by commas\n{USAGE}"
                    );
                    return Ended::Failed;
                }
            }
        }
    };
    let request = Export {
        format,
        path: out.to_owned(),
        replace,
        features,
    };
    in_session(
        sessions,
        file,
        "export",
        |refusal| refusal.said(file),
        |id| {
            let Response { model, result, .. } = sessions.export(id, request).wait();
            match result {
                Ok(done) => {
                    let mut stdout = std::io::stdout().lock();
                    let _ = writeln!(stdout, "wrote {} ({} bytes)", shown(&done.path), done.bytes);
                    for object in &done.objects {
                        let name = command::quoted(&object.name);
                        let _ = match &object.mesh {
                            Some(mesh) => writeln!(
                                stdout,
                                "{} {name}: {} triangles, {} vertices, volume {} mm³",
                                object.feature, mesh.triangles, mesh.vertices, mesh.volume
                            ),
                            None => writeln!(stdout, "{} {name}", object.feature),
                        };
                        if !object.left_out.is_empty() {
                            let _ = writeln!(
                                stdout,
                                "  left out as bounding no material: {}",
                                object
                                    .left_out
                                    .iter()
                                    .map(|n| n.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                    }
                    // The features asked for built, but a kernel fault stopped another: as
                    // `fmt` says it, the command couldn't wholly run.
                    if let Some(fault) = model.and_then(|read| read.fault) {
                        say!("{}: {}", shown(file), with_retry(&fault, "export"));
                        return Ended::Failed;
                    }
                    Ended::Clean
                }
                Err(problems) => Refusal::of_command("export", &problems)
                    .with_fault(model.and_then(|read| read.fault), "export")
                    .said(file),
            }
        },
    )
}

/// `silico cache clear FILE`: open, the session's `cache_clear`, close. Every entry of the
/// rebuild cache beside the file removed (the directory's, shared by every document in it,
/// decision 25), and only those: said on stderr, with how many, as `fmt` says what it wrote.
/// What it couldn't remove is each said, and the command couldn't run (exit 2), the rest
/// removed. Needs no kernel, and opens a file the gate refuses as any session does.
fn cache_clear(sessions: &Sessions<KernelPool>, file: &Path) -> Ended {
    in_session(
        sessions,
        file,
        "cache clear",
        |refusal| refusal.said(file),
        |id| match sessions.cache_clear(id).wait().result {
            Ok(cleared) => {
                let entries = if cleared.removed == 1 {
                    "entry"
                } else {
                    "entries"
                };
                say!(
                    "{}: removed {} {entries} from {}",
                    shown(file),
                    cleared.removed,
                    shown(Path::new(&cleared.directory))
                );
                Ended::Clean
            }
            Err(problems) => Refusal::of_command("cache clear", &problems).said(file),
        },
    )
}

/// `silico show FILE`: open, the session's `outline`, close; the outline on stdout. Needs no
/// kernel: it's the document as written, for reading and for `git diff`.
fn show(file: &Path) -> Ended {
    // A kernel and a cache it never uses: nothing is rebuilt, no worker is looked for, and
    // the user's cache secret is neither read nor made.
    let sessions = Sessions::with_cache_opener(no_kernel("show"), |dir| Cache::unsealed(dir));
    in_session(
        &sessions,
        file,
        "show",
        |refusal| refusal.said(file),
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
            Err(problems) => Refusal::of_command("show", &problems).said(file),
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

/// `value` as one line of JSON on stdout.
fn line(value: &impl Serialize) {
    match serde_json::to_string(value) {
        Ok(text) => {
            let _ = writeln!(std::io::stdout().lock(), "{text}");
        }
        Err(e) => say!("silico: a JSON line couldn't be written: {e}"),
    }
}

/// Each of `errors` on stderr, after the file it's about: its code and words, then the
/// feature, elements, and parameter at fault, and what to do (the error's own `Display`);
/// and how many more there are, of `total`, if not all are listed.
fn print_problems(file: &Path, errors: &[Error], total: u64) {
    for error in errors {
        say!("{}: {error}", shown(file));
    }
    let listed = errors.len() as u64;
    if total > listed {
        say!("{}: and {} more problems", shown(file), total - listed);
    }
}

/// Each of `warnings` on stderr, after the file it's about and marked as a warning: its
/// code and words, then the feature and elements it's about, and what to do (the warning's
/// own `Display`); and how many more there are, if not all are listed.
fn print_warnings(file: &Path, warnings: &Warnings) {
    for warning in warnings.listed() {
        say!("{}: warning: {warning}", shown(file));
    }
    let listed = warnings.listed().len() as u64;
    if warnings.total() > listed {
        say!(
            "{}: warning: and {} more warnings",
            shown(file),
            warnings.total() - listed
        );
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;
    use std::path::{Path, PathBuf};

    use silico_core::command::{NOT_RELOADED, Problems};
    use silico_core::{Error, ErrorCode};

    use super::{Ended, FailedLine, LineError, ProblemsLine, Refusal, with_retry};

    fn failed(ended: &Ended) -> bool {
        matches!(ended, Ended::Failed)
    }

    /// An error with every field of the contract set.
    fn every_field() -> Error {
        Error::new(ErrorCode::ReferenceBroken, "m \"q\"")
            .with_feature("k7f2qx3m")
            .with_elements(["Edge1", "Edge2"])
            .with_elements_total(5)
            .with_parameter("edges")
            .with_cause("c")
            .with_suggestion("s")
            .with_candidates(["Edge3"])
    }

    #[test]
    fn a_values_keys_keep_the_order_they_were_put_in() {
        // The workspace's serde_json has `preserve_order` (Cargo.toml says why): a `Value`'s
        // keys come out in the order they went in, in every build. Without it, they'd come
        // out sorted in a build where nothing else turns the feature on, as `-p silico-cli`
        // is: this fails then.
        let value = serde_json::json!({"zeta": 1, "alpha": {"mu": 2, "beta": 3}, "kappa": [4]});
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"zeta":1,"alpha":{"mu":2,"beta":3},"kappa":[4]}"#
        );
    }

    #[test]
    fn the_workspaces_serde_json_keeps_a_values_keys_in_order() {
        // The test above can only fail in a build where nothing else turns `preserve_order`
        // on, as `-p silico-cli` is; the workspace's build gets it from Rerun anyway. So the
        // declaration itself is checked too: dropped from Cargo.toml, this fails in every
        // build.
        let manifest =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .unwrap();
        let line = manifest
            .lines()
            .find(|line| line.starts_with("serde_json = "))
            .unwrap();
        assert!(line.contains(r#""preserve_order""#), "{line}");
    }

    #[test]
    fn check_jsons_lines_have_their_keys_sorted_in_any_build() {
        // tests/contract/check-line.json's order, every object's keys sorted, from the text
        // itself: serde_json's `Value` sorts them only in a build where nothing turns on its
        // `preserve_order` (Rerun does), so a line built from one came out in another order
        // in the workspace's build.
        let error = every_field();
        let errors = [error.clone(), Error::new(ErrorCode::InvalidInput, "n")];
        let problems =
            serde_json::to_string(&ProblemsLine::new(Path::new("a b.silico"), &errors, 7)).unwrap();
        assert_eq!(
            problems,
            concat!(
                r#"{"file":"a b.silico","ok":false,"problems":["#,
                r#"{"candidates":["Edge3"],"cause":"c","code":"reference_broken","#,
                r#""elements":["Edge1","Edge2"],"elements_total":5,"feature":"k7f2qx3m","#,
                r#""message":"m \"q\"","parameter":"edges","suggestion":"s"},"#,
                r#"{"candidates":[],"cause":null,"code":"invalid_input","elements":[],"#,
                r#""elements_total":0,"feature":null,"message":"n","parameter":null,"#,
                r#""suggestion":null}],"total":7}"#,
            )
        );
        let none =
            serde_json::to_string(&ProblemsLine::new(Path::new("ok.silico"), &[], 0)).unwrap();
        assert_eq!(
            none,
            r#"{"file":"ok.silico","ok":true,"problems":[],"total":0}"#
        );
        let failed =
            serde_json::to_string(&FailedLine::new(Path::new("m.silico"), &error)).unwrap();
        assert_eq!(
            failed,
            concat!(
                r#"{"error":{"candidates":["Edge3"],"cause":"c","code":"reference_broken","#,
                r#""elements":["Edge1","Edge2"],"elements_total":5,"feature":"k7f2qx3m","#,
                r#""message":"m \"q\"","parameter":"edges","suggestion":"s"},"#,
                r#""file":"m.silico","ok":false}"#,
            )
        );
    }

    #[test]
    fn a_lines_error_writes_every_field_the_core_writes() {
        // The line's error is the core's contract (Error's own Serialize), its keys sorted:
        // a field the core's error gains is one the line must write too.
        let error = every_field();
        let keys = |text: String| {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            let serde_json::Value::Object(map) = value else {
                panic!("{text}");
            };
            let mut keys: Vec<String> = map.into_iter().map(|(k, _)| k).collect();
            keys.sort();
            keys
        };
        let core = keys(serde_json::to_string(&error).unwrap());
        let line = keys(serde_json::to_string(&LineError::new(&error)).unwrap());
        assert_eq!(line, core);
        assert_eq!(core.len(), 9);
        // And each value as the core writes it.
        let core: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&error).unwrap()).unwrap();
        let line: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&LineError::new(&error)).unwrap()).unwrap();
        assert_eq!(line, core);
    }

    #[test]
    fn a_refusal_that_says_the_command_couldnt_run_is_exit_2_and_the_documents_is_1() {
        // Each kind docs/cli.md's exit 2 names: a file that couldn't be read or written, or
        // changed while it was rebuilt, a command cancelled, and a kernel fault (a shortage,
        // a crash); a document's problem, or its drift, is exit 1.
        for (code, couldnt) in [
            (ErrorCode::IoFailed, true),
            (ErrorCode::Conflict, true),
            (ErrorCode::Cancelled, true),
            (ErrorCode::ResourceExhausted, true),
            (ErrorCode::WorkerCrashed, true),
            (ErrorCode::InvalidInput, false),
            (ErrorCode::Drift, false),
        ] {
            let problems = Problems::from(Error::new(code, "m"));
            let refusal = Refusal::of_command("check", &problems);
            assert_eq!(failed(&refusal.ended()), couldnt, "{code:?}");
            let refusal = Refusal::of_open(Path::new("a.silico"), &problems);
            assert_eq!(failed(&refusal.ended()), couldnt, "{code:?}");
        }
    }

    #[test]
    fn opens_refusal_drops_the_path_its_line_names_and_a_commands_keeps_it() {
        // Open's `path` is the file each line names, so it's dropped; a later command's
        // parameter is its own (a document's key), so kept.
        let problems =
            Problems::from(Error::new(ErrorCode::InvalidInput, "too long").with_parameter("path"));
        let opened = Refusal::of_open(Path::new("a.silico"), &problems);
        assert_eq!(opened.errors[0].parameter(), None);
        assert!(!failed(&opened.ended()));
        let ran = Refusal::of_command("fmt", &problems);
        assert_eq!(ran.errors[0].parameter(), Some("path"));
        // A name that isn't UTF-8 is a file that couldn't be had, its path the
        // parameter at fault.
        let unnamed = PathBuf::from(OsString::from_vec(b"\xff.silico".to_vec()));
        let opened = Refusal::of_open(&unnamed, &problems);
        assert_eq!(opened.errors[0].parameter(), Some("path"));
        assert!(failed(&opened.ended()));
    }

    #[test]
    fn a_commands_refusal_about_its_file_says_how_to_go_on_in_the_clis_words() {
        // A conflict, or a session that couldn't take its file as it is now, says what
        // the CLI's user does (run it again), not what a session's client does; a save that
        // couldn't be written keeps the file's own advice.
        let said = |error: Error| {
            Refusal::of_command("fmt", &Problems::from(error)).errors[0]
                .suggestion()
                .map(str::to_owned)
        };
        let conflict = Error::new(ErrorCode::Conflict, "changed")
            .with_parameter("path")
            .with_suggestion("nothing was written: run format again, on the file as it is now");
        assert_eq!(
            said(conflict).as_deref(),
            Some("nothing was written: run `silico fmt` again, on the file as it is now")
        );
        let gone = Error::new(ErrorCode::IoFailed, "gone")
            .with_parameter("path")
            .with_cause(NOT_RELOADED)
            .with_suggestion("close the session and open the file where it is now");
        assert_eq!(
            said(gone).as_deref(),
            Some("check the file is there and run it again")
        );
        // R2-L2: a file grown past the limit is no matter of whether it's there: as open's
        // refusal of it says, with no session's advice.
        let grown = Error::new(ErrorCode::InvalidInput, "too long")
            .with_parameter("path")
            .with_cause(NOT_RELOADED)
            .with_suggestion("put the file back at a.silico, or close the session and open the file where it is now");
        assert_eq!(
            said(grown).as_deref(),
            Some("check it's the document you mean: no document is this long")
        );
        let unwritable = Error::new(ErrorCode::IoFailed, "couldn't be written")
            .with_parameter("path")
            .with_suggestion("check that this user may write the file");
        assert_eq!(
            said(unwritable).as_deref(),
            Some("check that this user may write the file")
        );
    }

    #[test]
    fn a_refusal_carrying_a_kernel_fault_is_exit_2_and_says_it_after_its_reasons() {
        // R2-L1: a document's refusal (exit 1 on its own) whose response's model met a
        // kernel fault is the command's failure, the fault said with the CLI's advice; the
        // fault is what stopped it, unless a reason says it couldn't run.
        let too_long = Problems::from(Error::new(ErrorCode::InvalidInput, "too long"));
        let fault = Error::new(ErrorCode::WorkerCrashed, "crashed").with_suggestion("report it");
        let plain = Refusal::of_command("fmt", &too_long).with_fault(None, "fmt");
        assert!(!failed(&plain.ended()));
        assert!(plain.stopped().is_none());
        let faulted = Refusal::of_command("fmt", &too_long).with_fault(Some(fault.clone()), "fmt");
        assert!(failed(&faulted.ended()));
        let stopped = faulted.stopped().unwrap();
        assert_eq!(
            (stopped.code(), stopped.suggestion()),
            (
                ErrorCode::WorkerCrashed,
                Some("report it; then run `silico fmt` to try it again")
            )
        );
        let cancelled = Problems::from(Error::new(ErrorCode::Cancelled, "cancelled"));
        let both = Refusal::of_command("check", &cancelled).with_fault(Some(fault), "check");
        assert_eq!(both.stopped().unwrap().code(), ErrorCode::Cancelled);
    }

    #[test]
    fn a_fault_the_refusal_lists_is_said_there_once_and_another_is_kept_beside_it() {
        // The refusal lists the fault itself (a feature it stopped, asked for): said there,
        // in the CLI's words, and not again. An error of the same code with another message,
        // or the same message with another code, is another error: kept, the fault said last.
        let fault =
            Error::new(ErrorCode::WorkerUnavailable, "no worker").with_suggestion("install it");
        let listed = Problems::from(fault.clone().with_suggestion("a session's advice"));
        let once = Refusal::of_command("export", &listed).with_fault(Some(fault.clone()), "export");
        assert!(once.fault.is_none());
        assert_eq!(once.errors.len(), 1);
        assert_eq!(
            once.errors[0].suggestion(),
            Some("install it; then run `silico export` to try it again")
        );
        assert!(failed(&once.ended()));
        for other in [
            Error::new(ErrorCode::WorkerUnavailable, "another worker"),
            Error::new(ErrorCode::WorkerCrashed, "no worker"),
        ] {
            let refusal = Refusal::of_command("export", &Problems::from(other.clone()))
                .with_fault(Some(fault.clone()), "export");
            assert_eq!(refusal.errors, std::slice::from_ref(&other), "{other:?}");
            assert_eq!(
                refusal.fault.as_ref().map(Error::message),
                Some("no worker"),
                "{other:?}"
            );
        }
    }

    #[test]
    fn a_fault_with_no_suggestion_of_its_own_gets_the_clis_retry_line_alone() {
        // R2-L5: a timeout says nothing of what to do; the CLI's line is all of it.
        let timeout = Error::new(ErrorCode::WorkerTimeout, "timed out");
        assert_eq!(
            with_retry(&timeout, "check").suggestion(),
            Some("run `silico check` to try it again")
        );
        let own = timeout.with_suggestion("simplify the part");
        assert_eq!(
            with_retry(&own, "check").suggestion(),
            Some("simplify the part; then run `silico check` to try it again")
        );
    }
}
