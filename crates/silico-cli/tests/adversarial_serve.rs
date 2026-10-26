//! `/break-it` on `silico serve` (docs/cli.md, docs/design/m4.9-mcp.md), the real binary on
//! the real kernel, as an MCP client starts it: every log on, garbage and calls on stdin,
//! and nothing on stdout but one JSON-RPC message a line; its stdout closed under it;
//! SIGTERM while its client never reads; stdin closed mid-transaction; SIGTERM mid-commit.
//! It never hangs, exits 0, and leaves the document as it was or as committed, whole, with
//! nothing beside it. Expected values: MCP's stdio rule (nothing but messages on stdout),
//! the note's rules, and the files as written here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::Value;

/// Longer than anything here takes on a loaded machine.
const HANG: Duration = Duration::from_secs(60);

/// A 40 × 20 × 10 box.
const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

const INITIALIZE: &str = r#"{"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "break-it", "version": "1"}}}"#;

/// `silico serve` in `dir`, its stdin and stdout piped, its stderr to `stderr`.
fn serve(dir: &Path, log: Option<&str>, stderr: Stdio) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_silico"));
    command
        .args(["serve", "--workspace"])
        .arg(dir)
        .env("SILICO_RENDER_WORKER", dir.join("no-such-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr);
    match log {
        Some(level) => command.env("SILICO_LOG", level),
        None => command.env_remove("SILICO_LOG"),
    };
    command.spawn().unwrap()
}

/// Each line of `child`'s stdout, as it comes.
fn lines(child: &mut Child) -> Receiver<String> {
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
fn ask(stdin: &mut ChildStdin, lines: &Receiver<String>, line: &str, id: &str) -> Value {
    writeln!(stdin, "{line}").unwrap();
    let id: Value = serde_json::from_str(id).unwrap();
    loop {
        let line = lines.recv_timeout(HANG).expect("no reply");
        let message = json_rpc(&line);
        if message.get("id") == Some(&id) {
            return message;
        }
    }
}

/// `line`, failing unless it's one JSON-RPC 2.0 message.
fn json_rpc(line: &str) -> Value {
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

/// How `child` ended, failing if it doesn't within [`HANG`].
fn exited(child: Child) -> ExitStatus {
    let (done, exited) = mpsc::channel();
    let mut child = child;
    std::thread::spawn(move || {
        let _ = done.send(child.wait());
    });
    match exited.recv_timeout(HANG) {
        Ok(status) => status.unwrap(),
        Err(_) => panic!("silico serve didn't end in {HANG:?}"),
    }
}

fn sigterm(child: &Child) {
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
}

/// The names in `dir` but the cache's.
fn listed(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".silico-cache")
        .collect();
    names.sort();
    names
}

#[test]
fn serve_writes_nothing_but_json_rpc_on_stdout_with_every_log_on() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.silico"), BOX).unwrap();
    let mut child = serve(dir.path(), Some("trace"), Stdio::piped());
    let mut stderr = child.stderr.take().unwrap();
    let logs = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    ask(&mut stdin, &lines, INITIALIZE, "0");
    // Garbage, then calls that reach the kernel, the cache, and the file system.
    for line in [
        &b"not json"[..],
        b"\xff\xfe",
        b"{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"tools/call\", \"params\": {\"name\": \"open\", \"arguments\": {\"path\": \"../x.silico\", \"create\": true}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 2, \"method\": \"tools/call\", \"params\": {\"name\": \"nope\"}}",
    ] {
        stdin.write_all(line).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    let opened = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "open", "arguments": {"path": "part.silico", "create": false}}}"#,
        "3",
    );
    assert_eq!(opened["result"]["isError"], false, "{opened}");
    // The open refused took s1, as each open takes the next ID when it's called.
    let session = opened["result"]["structuredContent"]["session"]["id"].clone();
    assert_eq!(session, "s2");
    let checked = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "check", "arguments": {"session": "s2", "model": "current", "fresh": false}, "_meta": {"progressToken": 1}}}"#,
        "4",
    );
    assert_eq!(checked["result"]["isError"], false, "{checked}");
    let measured = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "mass_properties", "arguments": {"session": "s2", "model": "current", "feature": "k7f2qx3m"}}}"#,
        "5",
    );
    assert_eq!(measured["result"]["isError"], false, "{measured}");
    // 40 × 20 × 10: 8,000 mm³, 2 × (800 + 400 + 200) = 2,800 mm², its centre half of each.
    assert_eq!(
        measured["result"]["structuredContent"]["result"]["bodies"],
        serde_json::json!([{"feature": "k7f2qx3m", "volume": 8000, "area": 2800, "center_of_mass": [20, 10, 5]}]),
        "{measured}"
    );
    drop(stdin);
    assert_eq!(exited(child).code(), Some(0));
    // Every line left on stdout is JSON-RPC too.
    while let Ok(line) = lines.recv_timeout(Duration::ZERO) {
        json_rpc(&line);
    }
    // The logs went to stderr.
    let logs = String::from_utf8_lossy(&logs.join().unwrap()).into_owned();
    assert!(logs.contains("serving MCP"), "{logs}");
}

#[test]
fn serve_ends_when_its_stdout_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    // The client's end of stdout closed before anything is written: the first reply's write
    // fails (EPIPE, SIGPIPE being ignored), and serve ends.
    drop(child.stdout.take());
    for id in 0..1_000 {
        let ping = format!("{{\"jsonrpc\": \"2.0\", \"id\": {id}, \"method\": \"ping\"}}\n");
        if stdin.write_all(ping.as_bytes()).is_err() {
            break;
        }
    }
    let status = exited(child);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn serve_ends_on_sigterm_while_its_client_never_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    // Read the handshake's answer only, so the handler's registered; then never read again.
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, "{INITIALIZE}").unwrap();
    let mut first = String::new();
    stdout.read_line(&mut first).unwrap();
    json_rpc(&first);
    // Far more pings than stdout's pipe and the server's queue hold, each with an ID of a
    // kilobyte, which its pong carries back: the writer blocks, then the reader, then this
    // thread's writes.
    let (wrote, written) = mpsc::channel();
    std::thread::spawn(move || {
        let pad = "p".repeat(1_000);
        for id in 0..100_000u64 {
            let ping =
                format!("{{\"jsonrpc\": \"2.0\", \"id\": \"{pad}{id}\", \"method\": \"ping\"}}\n");
            if stdin.write_all(ping.as_bytes()).is_err() {
                return;
            }
            if id == 150 {
                let _ = wrote.send(());
            }
        }
    });
    // 150 pongs of a kilobyte are more than stdout's pipe (64 KiB) holds, so the server's
    // writer is about to block, if it hasn't.
    written.recv_timeout(HANG).unwrap();
    sigterm(&child);
    let status = exited(child);
    assert_eq!(status.code(), Some(0), "{status:?}");
    drop(stdout);
}

#[test]
fn serve_rolls_back_a_transaction_when_its_stdin_closes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.silico"), BOX).unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    ask(&mut stdin, &lines, INITIALIZE, "0");
    for (id, name, arguments) in [
        (1, "open", r#"{"path": "part.silico", "create": false}"#),
        (2, "begin", r#"{"session": "s1"}"#),
        (
            3,
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
        ),
    ] {
        let reply = ask(
            &mut stdin,
            &lines,
            &format!(
                r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{name}", "arguments": {arguments}}}}}"#
            ),
            &id.to_string(),
        );
        assert_eq!(reply["result"]["isError"], false, "{reply}");
    }
    // A check running as the client goes away.
    writeln!(stdin, r#"{{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {{"name": "check", "arguments": {{"session": "s1", "model": "current", "fresh": false}}}}}}"#).unwrap();
    drop(stdin);
    assert_eq!(exited(child).code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("part.silico")).unwrap(),
        BOX
    );
    assert_eq!(listed(dir.path()), ["part.silico"]);
}

#[test]
fn serve_on_sigterm_mid_commit_leaves_the_document_whole() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.silico"), BOX).unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    ask(&mut stdin, &lines, INITIALIZE, "0");
    for (id, name, arguments) in [
        (1, "open", r#"{"path": "part.silico", "create": false}"#),
        (2, "begin", r#"{"session": "s1"}"#),
        (
            3,
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
        ),
    ] {
        let reply = ask(
            &mut stdin,
            &lines,
            &format!(
                r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{name}", "arguments": {arguments}}}}}"#
            ),
            &id.to_string(),
        );
        assert_eq!(reply["result"]["isError"], false, "{reply}");
    }
    // The commit sent, and SIGTERM at once: whichever wins, the file is whole.
    writeln!(stdin, r#"{{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {{"name": "commit", "arguments": {{"session": "s1"}}}}}}"#).unwrap();
    sigterm(&child);
    assert_eq!(exited(child).code(), Some(0));
    let text = std::fs::read_to_string(dir.path().join("part.silico")).unwrap();
    if text != BOX {
        let document: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(document["features"][0]["length"], 41, "{text}");
    }
    assert_eq!(listed(dir.path()), ["part.silico"]);
    drop(stdin);
}
