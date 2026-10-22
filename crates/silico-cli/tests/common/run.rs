//! Running `silico` in the CLI's tests: each run logged at debug (its log kept apart from
//! the stderr a test checks), given a cache secret made once (so a run makes none, and
//! writes nothing to disk it must sync, before it rebuilds), and held to a deadline under
//! nextest's, after which it's killed and the test is told where its time went: the run's
//! elapsed time, the longest waits between its log lines, and the log's last lines.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long one run may take: half of nextest's 120 s, so a run that stalls is told of
/// here, with its log, rather than killed with nothing to say.
pub const DEADLINE: Duration = Duration::from_secs(60);

/// A run's end: how it exited (none if it ran past its deadline and was killed), its
/// output, the stderr a test checks (without the debug log), its log, and how long it ran.
pub struct Finished {
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub log: Vec<String>,
    pub took: Duration,
    /// When it ended, as the log's clock reads (UTC, seconds into the day).
    pub ended: f64,
}

/// The secret every run's cache is given: 32 random bytes, made once for this process.
fn secret() -> &'static Path {
    static SECRET: OnceLock<PathBuf> = OnceLock::new();
    SECRET.get_or_init(|| {
        let file = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("cli-cache-secret-{}", std::process::id()));
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut bytes)
            .unwrap();
        std::fs::write(&file, bytes).unwrap();
        file
    })
}

/// Readies `cmd`, a run of `silico`: its cache's secret in place, if it names the cache's
/// home and has none yet (each test keeps its own home, so its caches stay its own); and its
/// log at debug, unless the test chose the log itself. Says whether the log is this
/// module's to keep apart.
pub fn ready(cmd: &mut Command) -> bool {
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    if let Some((_, Some(home))) = envs.iter().find(|(k, _)| k == "XDG_CACHE_HOME") {
        let file = Path::new(home).join("silico").join("cache-secret");
        if !file.exists() {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::copy(secret(), &file).unwrap();
        }
    }
    let value = |key: &str| {
        envs.iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.clone())
    };
    // This module's own setting, from a run readied before, is still its own.
    let ours = value("SILICO_LOG").as_deref() == Some(OUR_LOG)
        && value("SILICO_LOG_FORMAT").as_deref() == Some("text");
    if ours || value("SILICO_LOG").is_none() {
        cmd.env("SILICO_LOG", OUR_LOG)
            .env("SILICO_LOG_FORMAT", "text");
        return true;
    }
    false
}

/// The log filter this module runs `silico` with.
const OUR_LOG: &str = "debug";

/// Whether `line` begins a log record below a warning, as the text format writes one
/// (`2026-10-22T05:54:52.792014Z DEBUG …`): the records a default run wouldn't write.
fn quiet_record(line: &str) -> Option<bool> {
    let (time, rest) = line.split_once(' ')?;
    if !(time.len() > 20 && time.ends_with('Z') && time.as_bytes().get(10) == Some(&b'T')) {
        return None;
    }
    let level = rest.trim_start().split(' ').next()?;
    match level {
        "TRACE" | "DEBUG" | "INFO" => Some(true),
        "WARN" | "ERROR" => Some(false),
        _ => None,
    }
}

/// Waits for `child` up to `deadline`, killing it after. If `split`, the debug log its
/// stderr carries is kept apart from what a test checks: below-warning records go to `log`;
/// warnings, errors, and every other line (silico's own words) stay, as a default run
/// writes them. (A below-warning record of more than one line leaves its later lines in
/// stderr.)
pub fn finish(mut child: Child, deadline: Duration, split: bool) -> Finished {
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
    let took = start.elapsed();
    let ended = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() % 86_400.0);
    let all = String::from_utf8_lossy(&err.join().unwrap()).into_owned();
    let (mut stderr, mut log) = (String::new(), Vec::new());
    for line in all.split_inclusive('\n') {
        if split && quiet_record(line) == Some(true) {
            log.push(line.trim_end().to_owned());
            continue;
        }
        stderr.push_str(line);
    }
    Finished {
        status,
        stdout: out.join().unwrap(),
        stderr,
        log,
        took,
        ended,
    }
}

/// Where a run's time went: how long it ran, its five longest waits between log lines,
/// each with the line it waited after and the line that ended it, and its last 30 lines.
pub fn diagnosis(f: &Finished) -> String {
    let seconds = |line: &str| -> Option<f64> {
        let time = line.split(' ').next()?;
        let clock = time.split('T').nth(1)?.trim_end_matches('Z');
        let mut parts = clock.split(':');
        let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
        Some(
            h.parse::<f64>().ok()? * 3600.0
                + m.parse::<f64>().ok()? * 60.0
                + s.parse::<f64>().ok()?,
        )
    };
    let timed: Vec<(f64, &String)> = f
        .log
        .iter()
        .filter_map(|l| seconds(l).map(|t| (t, l)))
        .collect();
    let mut waits: Vec<(f64, &String, &String)> = timed
        .windows(2)
        .map(|w| (w[1].0 - w[0].0, w[0].1, w[1].1))
        .collect();
    waits.sort_by(|a, b| b.0.total_cmp(&a.0));
    let silent = timed.last().map_or(String::new(), |(t, _)| {
        format!(", {:.3} s of it since its last log line", f.ended - t)
    });
    let mut text = format!(
        "ran {:?}{silent}{}; {} log lines\nlongest waits:\n",
        f.took,
        if f.status.is_none() {
            ", past its deadline: killed"
        } else {
            ""
        },
        f.log.len()
    );
    for (wait, after, before) in waits.iter().take(5) {
        text.push_str(&format!(
            "  {wait:.3} s after\n    {after}\n  until\n    {before}\n"
        ));
    }
    text.push_str("last lines:\n");
    for line in &f.log[f.log.len().saturating_sub(30)..] {
        text.push_str(&format!("  {line}\n"));
    }
    text
}

/// `cmd`'s output, as `Command::output` gives it, but readied ([`ready`]), its stderr
/// without the debug log, and the run held to [`DEADLINE`]: one past it fails the test,
/// saying where its time went.
#[track_caller]
pub fn output(mut cmd: Command) -> Output {
    let split = ready(&mut cmd);
    let child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let f = finish(child, DEADLINE, split);
    let Some(status) = f.status else {
        panic!("silico ran past {DEADLINE:?}: {}", diagnosis(&f));
    };
    Output {
        status,
        stdout: f.stdout,
        stderr: f.stderr.into_bytes(),
    }
}
