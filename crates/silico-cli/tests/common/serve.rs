//! Running `silico serve` in the CLI's tests as an MCP client starts it: its stdin and
//! stdout piped, its stdout's lines as they come, each checked to be one JSON-RPC message,
//! requests sent and their replies waited for, and its end waited for, each failing past
//! what's left of the test's [`BUDGET`] rather than hanging.
// Each test program that includes this uses only some of it.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Instant;

use serde_json::Value;

use crate::run::{BUDGET, began, left};

/// A 40 × 20 × 10 box.
pub const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// `initialize`, as request 0.
pub const INITIALIZE: &str = r#"{"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}"#;

/// `silico serve` in the workspace `dir`, its stdin and stdout piped, its stderr to
/// `stderr`, its log at `log` (or the default), with the workers beside it (the render
/// worker started only by a render).
pub fn serve(dir: &Path, log: Option<&str>, stderr: Stdio) -> Child {
    began();
    let mut command = Command::new(env!("CARGO_BIN_EXE_silico"));
    command
        .args(["serve", "--workspace"])
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr);
    match log {
        Some(level) => command.env("SILICO_LOG", level),
        None => command.env_remove("SILICO_LOG"),
    };
    command.spawn().unwrap()
}

/// `silico serve` in the workspace `dir` as [`serve`] starts it, but with the real workers
/// (the ones built beside it), no log, and its cache's secret in the cache home `home`,
/// written as [`give_secret`](crate::run::give_secret) writes one.
pub fn serve_working(dir: &Path, home: &Path, stderr: Stdio) -> Child {
    began();
    crate::run::give_secret(&home.join("silico").join("cache-secret"));
    let silico = Path::new(env!("CARGO_BIN_EXE_silico"));
    for worker in [
        "silico-kernel-worker",
        "silico-sketch-worker",
        "silico-render-worker",
        "silico-step-worker",
    ] {
        assert!(
            silico.with_file_name(worker).is_file(),
            "{worker} isn't built beside silico: pixi run cargo build -p silico-kernel -p silico-sketch -p silico-render --bins"
        );
    }
    Command::new(silico)
        .args(["serve", "--workspace"])
        .arg(dir)
        .env("XDG_CACHE_HOME", home)
        .env_remove("SILICO_LOG")
        .env_remove("SILICO_KERNEL_WORKER")
        .env_remove("SILICO_SKETCH_WORKER")
        .env_remove("SILICO_RENDER_WORKER")
        .env_remove("SILICO_STEP_WORKER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .unwrap()
}

/// Each line of `child`'s stdout, as it comes.
pub fn lines(child: &mut Child) -> Receiver<String> {
    let (to, lines) = mpsc::channel();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    std::thread::spawn(move || {
        for line in stdout.lines() {
            match line {
                Ok(line) => {
                    let _ = to.send(line);
                }
                Err(e) => {
                    let _ = to.send(format!("NOT UTF-8: {e}"));
                    return;
                }
            }
        }
    });
    lines
}

/// Sends `line`, and gives the reply whose ID is `id` (JSON text), every line before it
/// checked to be JSON-RPC.
pub fn ask(stdin: &mut ChildStdin, lines: &Receiver<String>, line: &str, id: &str) -> Value {
    json_rpc(&ask_line(stdin, lines, line, id))
}

/// [`ask`], the reply as written.
pub fn ask_line(stdin: &mut ChildStdin, lines: &Receiver<String>, line: &str, id: &str) -> String {
    let method = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|sent| sent["method"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "no method".to_owned());
    writeln!(stdin, "{line}").unwrap();
    let id: Value = serde_json::from_str(id).unwrap();
    let asked = Instant::now();
    let until = asked + left();
    let mut before = 0;
    loop {
        let line = match lines.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => panic!(
                "silico serve sent no reply to request {id} ({method}) in {:?}, when the test's \
                 {BUDGET:?} ran out ({:?} since it began): it's hung; {before} other messages came \
                 meanwhile",
                asked.elapsed(),
                began().elapsed()
            ),
            Err(RecvTimeoutError::Disconnected) => panic!(
                "silico serve's stdout closed {:?} after request {id} ({method}), with no reply \
                 to it: the server ended; {before} other messages came meanwhile",
                asked.elapsed()
            ),
        };
        let message = json_rpc(&line);
        if message.get("id") == Some(&id) {
            return line;
        }
        before += 1;
    }
}

/// `line`, failing unless it's one JSON-RPC 2.0 message.
pub fn json_rpc(line: &str) -> Value {
    let message: Value =
        serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON on stdout ({e}): {line}"));
    assert_eq!(message["jsonrpc"], "2.0", "{line}");
    assert!(
        message.get("result").is_some()
            || message.get("error").is_some()
            || message["method"] == "notifications/progress",
        "{line}"
    );
    message
}

/// How `child` ended, failing if it doesn't within what's left of the test's [`BUDGET`].
pub fn exited(mut child: Child) -> ExitStatus {
    let (done, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait());
    });
    let asked = Instant::now();
    match exited.recv_timeout(left()) {
        Ok(status) => status.unwrap(),
        Err(_) => panic!(
            "silico serve didn't end in {:?}, when the test's {BUDGET:?} ran out ({:?} since it \
             began): it's hung",
            asked.elapsed(),
            began().elapsed()
        ),
    }
}

pub fn sigterm(child: &Child) {
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
}

/// The names in `dir` but the cache's.
pub fn listed(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".silico-cache")
        .collect();
    names.sort();
    names
}
