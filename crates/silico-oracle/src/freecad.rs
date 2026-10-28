//! FreeCAD's side: every case built and measured in one run of headless FreeCAD
//! (`oracle.py`), and any other FreeCAD script run the same way ([`script`]). FreeCAD runs
//! with a settings directory of its own, so neither the user's preferences (which change
//! what some features build) nor anything it writes are shared with the FreeCAD they use.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::compare::Outcome;

/// Names a `freecadcmd` to use instead of the workspace's pixi `oracle` environment's.
pub const FREECADCMD_ENV: &str = "SILICO_FREECADCMD";

const ORACLE: &str = include_str!("oracle.py");

/// What `oracle.py` prints once it has read the cases, and before each case's outcome.
const READY: &str = "SILICO_ORACLE_READY";
const CASE: &str = "SILICO_ORACLE_CASE ";

/// What a [`script`] prints before its value.
const VALUE: &str = "SILICO_ORACLE ";

/// How long FreeCAD may go without giving what it was asked for (a case's outcome, or
/// word that it has read the cases) before it's taken to have hung, and is killed: the
/// case is then its failure, as a crash is. Its other output doesn't count, so a FreeCAD
/// that hangs printing progress is killed too.
pub const CASE_DEADLINE: Duration = Duration::from_secs(120);

/// Names another deadline, in whole seconds (for tests of a hang).
pub const CASE_DEADLINE_ENV: &str = "SILICO_ORACLE_CASE_DEADLINE";

/// How many of FreeCAD's last lines a report of how it ended keeps, and how many bytes of
/// each.
const TAIL_LINES: usize = 20;
const TAIL_LINE_BYTES: usize = 1000;

/// The most of a line of FreeCAD's that's read, in bytes; the rest is dropped. A case's
/// outcome is a few hundred bytes, a script's value at most tens of kilobytes.
const MAX_LINE: usize = 1 << 20;

/// How many lines may wait to be read. A FreeCAD that writes faster than that blocks on
/// its output, and is killed at its deadline, rather than filling memory.
const BACKLOG: usize = 1024;

fn case_deadline() -> Duration {
    std::env::var(CASE_DEADLINE_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(CASE_DEADLINE, Duration::from_secs)
}

/// `freecadcmd`: the one [`FREECADCMD_ENV`] names, or the workspace's pixi `oracle`
/// environment's. A missing oracle is an error, never a skip: a test that quietly didn't
/// compare against FreeCAD would look like one that did.
pub fn freecadcmd() -> Result<PathBuf, String> {
    let exe = std::env::var_os(FREECADCMD_ENV).map_or_else(
        || {
            PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../.pixi/envs/oracle/bin/freecadcmd"
            ))
        },
        PathBuf::from,
    );
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(format!(
            "the FreeCAD oracle isn't installed: run `pixi install -e oracle` ({} is missing)",
            exe.display()
        ))
    }
}

/// What FreeCAD builds and measures for each of `cases` (each a [`Case`](crate::Case), or
/// anything that writes as one does: a name and a shape `oracle.py` builds), in order. A
/// case FreeCAD crashes or hangs on is [`Outcome::Lost`], saying how FreeCAD ended and what
/// it last wrote, and the rest run in a new FreeCAD.
pub fn run<C: serde::Serialize>(cases: &[C]) -> Result<Vec<Outcome>, String> {
    run_as(cases, Outcome::Lost)
}

/// What FreeCAD gives for each of `cases`, in order, as [`run`] runs them, each outcome
/// read as an `O`: `lost` makes the outcome of a case FreeCAD crashed or hung on, from how
/// it ended.
pub fn run_as<C: serde::Serialize, O: serde::de::DeserializeOwned>(
    cases: &[C],
    lost: fn(String) -> O,
) -> Result<Vec<O>, String> {
    let scratch = Scratch::new()?;
    let script = scratch.write("oracle.py", ORACLE)?;
    let json = serde_json::to_string(cases).map_err(|e| format!("can't write the cases: {e}"))?;
    let input = scratch.write("cases.json", &json)?;
    let mut outcomes = Vec::with_capacity(cases.len());
    while outcomes.len() < cases.len() {
        let first = outcomes.len().to_string();
        let env = [
            ("SILICO_ORACLE_CASES", input.as_os_str()),
            ("SILICO_ORACLE_FIRST", OsStr::new(&first)),
        ];
        let mut freecad = Freecad::start(&scratch, &script, &env)?;
        let mut started = false;
        let hung = loop {
            match freecad.next() {
                Next::Line(line) => {
                    // A marker can follow FreeCAD's own progress output on its line.
                    if line.contains(READY) {
                        started = true;
                        freecad.answered();
                    } else if let Some(at) = line.find(CASE) {
                        let outcome = serde_json::from_str(&line[at + CASE.len()..])
                            .map_err(|e| format!("FreeCAD's outcome can't be read: {e}"))?;
                        outcomes.push(outcome);
                        freecad.answered();
                    } else {
                        freecad.keep(line);
                    }
                }
                Next::Closed => break false,
                Next::Hung => break true,
            }
        };
        let ended = freecad.stop(hung)?;
        if !started {
            return Err(ended.why("before starting the cases"));
        }
        if outcomes.len() > cases.len() {
            return Err(format!(
                "FreeCAD gave {} outcomes for {} cases",
                outcomes.len(),
                cases.len()
            ));
        }
        if outcomes.len() < cases.len() {
            outcomes.push(lost(ended.why("building it")));
        }
    }
    Ok(outcomes)
}

/// Runs `source`, a Python script with FreeCAD's modules importable, in headless FreeCAD,
/// and returns the value it printed as `print("SILICO_ORACLE " + json.dumps(value))`. An
/// error, saying how FreeCAD ended and what it last wrote, if it ended or went
/// [`CASE_DEADLINE`] without printing one.
pub fn script(source: &str) -> Result<serde_json::Value, String> {
    let scratch = Scratch::new()?;
    let path = scratch.write("script.py", source)?;
    let mut freecad = Freecad::start(&scratch, &path, &[])?;
    let hung = loop {
        match freecad.next() {
            Next::Line(line) => match line.find(VALUE) {
                Some(at) => {
                    return serde_json::from_str(&line[at + VALUE.len()..])
                        .map_err(|e| format!("the script's value can't be read: {e}"));
                }
                None => freecad.keep(line),
            },
            Next::Closed => break false,
            Next::Hung => break true,
        }
    };
    Err(freecad
        .stop(hung)?
        .why("before the script printed its value"))
}

/// A directory of FreeCAD's own for one run: its settings (`home`), its script, and its
/// input. Removed when dropped.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new() -> Result<Scratch, String> {
        static RUNS: AtomicU32 = AtomicU32::new(0);
        let run = RUNS.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("silico-oracle-{}-{run}", std::process::id()));
        let home = dir.join("home");
        std::fs::create_dir_all(&home)
            .map_err(|e| format!("can't make {}: {e}", home.display()))?;
        Ok(Scratch(dir))
    }

    pub(crate) fn write(&self, name: &str, contents: &str) -> Result<PathBuf, String> {
        let path = self.0.join(name);
        std::fs::write(&path, contents)
            .map_err(|e| format!("can't write {}: {e}", path.display()))?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// FreeCAD running a script, its output read line by line. Killed if it's dropped still
/// running.
struct Freecad {
    child: Child,
    lines: mpsc::Receiver<String>,
    /// When it last gave what it was asked for (or started).
    since: Instant,
    /// Its last lines since then.
    tail: VecDeque<String>,
}

enum Next {
    Line(String),
    /// It closed its output: it's ending.
    Closed,
    /// It went [`CASE_DEADLINE`] without an answer.
    Hung,
}

/// How a run of FreeCAD ended, and what it last wrote.
struct Ended {
    hung: bool,
    status: ExitStatus,
    tail: Vec<String>,
}

impl Ended {
    /// Why FreeCAD gave no answer, `when`.
    fn why(&self, when: &str) -> String {
        let mut why = if self.hung {
            format!(
                "FreeCAD took longer than {} s {when}",
                case_deadline().as_secs()
            )
        } else if self.status.success() {
            format!("FreeCAD stopped {when} ({})", self.status)
        } else {
            format!("FreeCAD crashed {when} ({})", self.status)
        };
        if !self.tail.is_empty() {
            why = format!("{why}; its last output:\n{}", self.tail.join("\n"));
        }
        why
    }
}

/// FreeCAD's command for `script`, its settings and caches in `home`, in the C locale.
fn command(exe: PathBuf, script: &Path, home: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .arg(script)
        .env("FREECAD_USER_HOME", home)
        .env("HOME", home)
        .env("CFFIXED_USER_HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        // One locale on every platform, whatever the caller's: what OCCT's C library calls
        // answer can depend on it (`isgraph` past ASCII, which macOS's UTF-8 locales accept
        // and its C locale and glibc's don't), and an oracle answers alike everywhere. So
        // FreeCAD's Python is ASCII for its stdout and for files opened without an encoding
        // (its interpreter is isolated, so no PYTHONUTF8 or PYTHONIOENCODING reaches it):
        // scripts print only through `json.dumps`, which writes ASCII, and open their
        // input as UTF-8.
        .env("LC_ALL", "C")
        .env("LANG", "C");
    command
}

impl Freecad {
    fn start(scratch: &Scratch, script: &Path, env: &[(&str, &OsStr)]) -> Result<Freecad, String> {
        // Its stdout and stderr share one pipe, so its lines come in the order it wrote
        // them, whatever the threads reading them do.
        let pipe = |e: std::io::Error| format!("can't make a pipe for FreeCAD's output: {e}");
        let (output, input) = std::io::pipe().map_err(pipe)?;
        // A home of its own: its settings (FREECAD_USER_HOME), and what it keeps in the
        // user's home besides, its caches among them (found through CFFIXED_USER_HOME on
        // macOS, HOME and the XDG directories elsewhere).
        let home = scratch.0.join("home");
        let mut command = command(freecadcmd()?, script, &home);
        command
            .stdin(Stdio::null())
            .stdout(input.try_clone().map_err(pipe)?)
            .stderr(input);
        for (name, value) in env {
            command.env(name, value);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("can't run FreeCAD: {e}"))?;
        // The command holds this end of the pipe too: the output ends only when it's gone.
        drop(command);
        let lines = read_lines(output);
        Ok(Freecad {
            child,
            lines,
            since: Instant::now(),
            tail: VecDeque::new(),
        })
    }

    /// Its next line, or that it ended.
    fn next(&mut self) -> Next {
        // Checked first: a line already waiting is handed out even past the deadline, so a
        // FreeCAD that floods its output would never be found hung.
        let left = case_deadline().saturating_sub(self.since.elapsed());
        if left.is_zero() {
            return Next::Hung;
        }
        match self.lines.recv_timeout(left) {
            Ok(line) => Next::Line(line),
            Err(mpsc::RecvTimeoutError::Disconnected) => Next::Closed,
            Err(mpsc::RecvTimeoutError::Timeout) => Next::Hung,
        }
    }

    /// It gave what it was asked for: its deadline starts again, and its output so far is
    /// no longer of interest.
    fn answered(&mut self) {
        self.since = Instant::now();
        self.tail.clear();
    }

    /// Keeps `line` among its last lines.
    fn keep(&mut self, mut line: String) {
        if line.len() > TAIL_LINE_BYTES {
            let mut end = TAIL_LINE_BYTES;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        if self.tail.len() == TAIL_LINES {
            self.tail.pop_front();
        }
        self.tail.push_back(line);
    }

    /// Waits for it to exit (killing it first if it `hung`, or if it's still running at its
    /// deadline, its output closed), and says how it ended.
    fn stop(&mut self, hung: bool) -> Result<Ended, String> {
        let mut hung = hung;
        while !hung
            && self
                .child
                .try_wait()
                .map_err(|e| format!("can't wait for FreeCAD: {e}"))?
                .is_none()
        {
            if self.since.elapsed() >= case_deadline() {
                hung = true;
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if hung {
            let _ = self.child.kill();
        }
        let status = self
            .child
            .wait()
            .map_err(|e| format!("can't wait for FreeCAD: {e}"))?;
        Ok(Ended {
            hung,
            status,
            tail: self.tail.drain(..).collect(),
        })
    }
}

impl Drop for Freecad {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// FreeCAD's output, line by line, from a thread that ends when it's closed. Lines are read
/// as bytes, so text that isn't UTF-8 can't stop the reading, and cut at [`MAX_LINE`].
fn read_lines(output: std::io::PipeReader) -> mpsc::Receiver<String> {
    let (send, lines) = mpsc::sync_channel(BACKLOG);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut reader)
                .take(MAX_LINE as u64)
                .read_until(b'\n', &mut line)
            {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if line.len() == MAX_LINE && !line.ends_with(b"\n") {
                // Cut: drop the rest of the line, so it doesn't arrive as more lines.
                let _ = reader.skip_until(b'\n');
            }
            let text = String::from_utf8_lossy(&line);
            if send
                .send(text.trim_end_matches(['\n', '\r']).to_owned())
                .is_err()
            {
                return;
            }
        }
    });
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freecad_runs_in_the_c_locale_whatever_the_callers() {
        let command = command(
            PathBuf::from("freecadcmd"),
            Path::new("s.py"),
            Path::new("h"),
        );
        let envs: Vec<_> = command.get_envs().collect();
        for name in ["LC_ALL", "LANG"] {
            assert!(
                envs.contains(&(OsStr::new(name), Some(OsStr::new("C")))),
                "{name}: {envs:?}"
            );
        }
    }

    /// FreeCAD reads the scripts it's given in the locale's encoding, ASCII on CI (a hook
    /// that runs `oracle.py` failed on a "π" in a comment): every script the oracle hands it
    /// is ASCII.
    #[test]
    fn the_scripts_freecad_runs_are_ascii() {
        for (name, text) in [
            ("oracle.py", super::ORACLE),
            ("the sketch oracle's script", crate::sketch::SCRIPT),
        ] {
            let first = text.char_indices().find(|(_, c)| !c.is_ascii());
            assert_eq!(first, None, "{name} isn't ASCII");
        }
    }
}
