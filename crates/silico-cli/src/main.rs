//! The `silico` command-line client: `check`, `fmt`, `show`, and `schema`, each a thin client
//! of the core's commands (`silico_core::command`), which MCP and the app run too, and
//! `textconv`, for `git diff` (docs/cli.md). `check`, `fmt`, and `show` each open a file as a
//! session, run one session command on it (`check`, `format`, `outline`), and close it
//! (docs/design/m4.1-command-api.md, section 9), every file on one kernel pool and one
//! sketch solver.
//!
//! Exit codes: 0, nothing wrong; 1, a document has problems (or, with `fmt --check`, isn't
//! in canonical form, or a file is longer than a document may be); 2, the command couldn't
//! run (a usage error, a file that couldn't be read or written, or whose name isn't UTF-8,
//! a file that changed while `fmt` rebuilt it, or a kernel or sketch solver that couldn't
//! start or failed in a way that isn't the document's: a kernel fault, said after the
//! document's problems).

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use silico_core::cache::Cache;
use silico_core::command::{self, ModelChoice, Problems, Response, SessionId, Sessions};
use silico_core::file::opened_and_read;
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
        (Command::Schema, []) => schema("document"),
        (Command::Schema, [name]) => schema(&name.to_string_lossy()),
        (_, []) | (Command::Show | Command::Textconv | Command::Schema, [_, _, ..]) => {
            say!("{USAGE}");
            Ended::Failed
        }
        (Command::Check { json }, _) => each(&|sessions, file| check(sessions, file, json)),
        (Command::Fmt { check }, _) => each(&|sessions, file| fmt(sessions, file, check)),
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
/// all counted, and whether it says the command couldn't run (exit 2), not that the
/// document, or the file, has problems (exit 1).
struct Refusal {
    errors: Vec<Error>,
    total: u64,
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
    /// session for each run, not as a session's client goes on.
    fn of_command(command: &str, problems: &Problems) -> Self {
        let errors = problems
            .errors()
            .iter()
            .map(|error| {
                let about_the_file = error.parameter() == Some("path");
                if about_the_file && error.code() == ErrorCode::Conflict {
                    error.clone().with_suggestion(format!(
                        "nothing was written: run `silico {command}` again, on the file as it \
                         is now"
                    ))
                } else if about_the_file && error.cause() == Some(command::NOT_RELOADED) {
                    error
                        .clone()
                        .with_suggestion("check the file is there and run it again")
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
            failed,
        }
    }

    fn ended(&self) -> Ended {
        if self.failed {
            Ended::Failed
        } else {
            Ended::Problems
        }
    }

    /// Each reason on stderr, after `file`.
    fn said(&self, file: &Path) -> Ended {
        print_problems(file, &self.errors, self.total);
        self.ended()
    }

    /// `check --json`'s line for `file`: the reason it couldn't run, or the problems.
    fn line(&self, file: &Path) -> Ended {
        let stopped = self
            .errors
            .iter()
            .find(|error| couldnt_run(error))
            .or_else(|| self.errors.first().filter(|_| self.failed));
        match stopped {
            Some(error) => failed_line(file, error),
            None => {
                problems_line(file, &self.errors, self.total);
                Ended::Problems
            }
        }
    }
}

/// `fault`, a kernel fault the rebuild of a file met (its suggestion the worker's own), as
/// `silico command` says it: then, to run the command again, which rebuilds the file anew.
fn retried(fault: &Error, command: &str) -> Error {
    let again = format!("run `silico {command}` again to retry it");
    let suggestion = match fault.suggestion() {
        Some(own) => format!("{own}; {again}"),
        None => again,
    };
    fault.clone().with_suggestion(suggestion)
}

/// `silico check FILE`: open, the session's `check` of its current revision, close. Its
/// problems (failed features and sketches, and drift of fingerprints and of sketches' stored
/// solves), each on stderr, and a kernel fault the rebuild met after them; or, with
/// `--json`, one line of JSON on stdout for each file: `{"file", "ok", "problems", "total"}`,
/// each problem an error as the core's error contract has it, or, for a file that couldn't
/// be checked (a refusal that says it couldn't run, or a kernel fault), `{"file", "ok":
/// false, "error"}`, the error that stopped it, in the same shape
/// (tests/contract/check-line.json).
fn check(sessions: &Sessions<KernelPool>, file: &Path, json: bool) -> Ended {
    let refused = |refusal: Refusal| {
        if json {
            refusal.line(file)
        } else {
            refusal.said(file)
        }
    };
    in_session(sessions, file, "check", refused, |id| {
        let Response { model, result, .. } = sessions.check(id, ModelChoice::Current).wait();
        let problems = match result {
            Ok(answer) => answer.problems,
            Err(problems) => return refused(Refusal::of_command("check", &problems)),
        };
        // A kernel fault isn't the document's: given after its problems, and the check's
        // failure (exit 2), as a file that couldn't be checked.
        if let Some(fault) = model.and_then(|read| read.fault) {
            let fault = retried(&fault, "check");
            if json {
                return failed_line(file, &fault);
            }
            print_problems(file, problems.errors(), problems.total());
            say!("{}: {fault}", shown(file));
            return Ended::Failed;
        }
        if json {
            problems_line(file, problems.errors(), problems.total());
        } else {
            print_problems(file, problems.errors(), problems.total());
        }
        Ended::of(&problems)
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
fn fmt(sessions: &Sessions<KernelPool>, file: &Path, check_only: bool) -> Ended {
    in_session(
        sessions,
        file,
        "fmt",
        |refusal| refusal.said(file),
        |id| {
            let Response { model, result, .. } = sessions.format(id, None, !check_only).wait();
            let formatted = match result {
                Ok(formatted) => formatted,
                Err(problems) => return Refusal::of_command("fmt", &problems).said(file),
            };
            let problems = &formatted.problems;
            print_problems(file, problems.errors(), problems.total());
            let mut ended = Ended::of(problems);
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
            if let Some(fault) = model.and_then(|read| read.fault) {
                say!("{}: {}", shown(file), retried(&fault, "fmt"));
                ended = Ended::Failed;
            }
            ended
        },
    )
}

/// `silico show FILE`: open, the session's `outline`, close; the outline on stdout. Needs no
/// kernel: it's the document as written, for reading and for `git diff`.
fn show(file: &Path) -> Ended {
    // A kernel and a cache it never uses: nothing is rebuilt, no worker is looked for, and
    // the user's cache secret is neither read nor made.
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

fn line(value: serde_json::Value) {
    let _ = writeln!(std::io::stdout().lock(), "{value}");
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;
    use std::path::{Path, PathBuf};

    use silico_core::command::{NOT_RELOADED, Problems};
    use silico_core::{Error, ErrorCode};

    use super::{Ended, Refusal};

    fn failed(ended: &Ended) -> bool {
        matches!(ended, Ended::Failed)
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
        // Q-M: open's `path` is the file each line names; a later command's parameter is
        // its own (a document's key), so kept.
        let problems =
            Problems::from(Error::new(ErrorCode::InvalidInput, "too long").with_parameter("path"));
        let opened = Refusal::of_open(Path::new("a.silico"), &problems);
        assert_eq!(opened.errors[0].parameter(), None);
        assert!(!failed(&opened.ended()));
        let ran = Refusal::of_command("fmt", &problems);
        assert_eq!(ran.errors[0].parameter(), Some("path"));
        // R-M2: a name that isn't UTF-8 is a file that couldn't be had, its path the
        // parameter at fault.
        let unnamed = PathBuf::from(OsString::from_vec(b"\xff.silico".to_vec()));
        let opened = Refusal::of_open(&unnamed, &problems);
        assert_eq!(opened.errors[0].parameter(), Some("path"));
        assert!(failed(&opened.ended()));
    }

    #[test]
    fn a_commands_refusal_about_its_file_says_how_to_go_on_in_the_clis_words() {
        // R-M1: a conflict, or a session that couldn't take its file as it is now, says what
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
        let unwritable = Error::new(ErrorCode::IoFailed, "couldn't be written")
            .with_parameter("path")
            .with_suggestion("check that this user may write the file");
        assert_eq!(
            said(unwritable).as_deref(),
            Some("check that this user may write the file")
        );
    }
}
