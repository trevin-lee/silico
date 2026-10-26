//! `silico serve` (docs/cli.md, docs/design/m4.9-mcp.md): the real binary, started as an
//! MCP client starts it, in a temporary workspace: it initializes, lists its tools, makes a
//! new document, closes it, and exits 0 when its stdin ends, nothing on stdout but MCP's
//! messages; and it refuses a disk's root as its workspace. Expected values: MCP
//! 2025-06-18's shapes, the tool count from the design's command list, and the file made.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

/// Longer than anything here takes on a loaded machine.
const HANG: Duration = Duration::from_secs(60);

#[test]
fn serve_initializes_lists_its_tools_makes_a_document_and_ends_with_its_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_silico"))
        .args(["serve", "--workspace"])
        .arg(dir.path())
        // A render worker that isn't there: serve still starts, refusing only render.
        .env("SILICO_RENDER_WORKER", dir.path().join("no-such-worker"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let (lines, replies) = mpsc::channel();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    std::thread::spawn(move || {
        for line in stdout.lines() {
            let _ = lines.send(line.unwrap());
        }
    });
    let request = |stdin: &mut std::process::ChildStdin, line: &str| -> Value {
        writeln!(stdin, "{line}").unwrap();
        let reply = replies.recv_timeout(HANG).expect("no reply");
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"))
    };
    let init = request(
        &mut stdin,
        r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}}}"#,
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "silico");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains(&format!("The workspace is {}:", root.display())),
        "{instructions}"
    );
    writeln!(
        stdin,
        r#"{{"jsonrpc": "2.0", "method": "notifications/initialized"}}"#
    )
    .unwrap();
    let tools = request(
        &mut stdin,
        r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/list"}"#,
    );
    let tools = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 23);
    assert_eq!(tools[1]["name"], "open");
    let opened = request(
        &mut stdin,
        r#"{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "open", "arguments": {"path": "new.silico", "create": true}}}"#,
    );
    let envelope = &opened["result"]["structuredContent"];
    assert_eq!(envelope["ok"], true, "{opened}");
    assert_eq!(envelope["session"]["id"], "s1");
    assert_eq!(envelope["result"]["created"], true);
    assert!(root.join("new.silico").is_file());
    let closed = request(
        &mut stdin,
        r#"{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "close", "arguments": {"session": "s1"}}}"#,
    );
    assert_eq!(closed["result"]["isError"], false, "{closed}");
    let version = request(
        &mut stdin,
        r#"{"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "version"}}"#,
    );
    assert_eq!(version["result"]["structuredContent"]["result"]["api"], 1);
    drop(stdin);
    let (done, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait_with_output());
    });
    let out = exited
        .recv_timeout(HANG)
        .expect("serve didn't end")
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("SILICO_RENDER_WORKER names"), "{stderr}");
}

#[test]
fn serve_refuses_the_root_of_the_disk_as_its_workspace() {
    for args in [
        &["serve", "--workspace", "/"][..],
        &["serve", "--workspace=/"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_silico"))
            .args(args)
            .env_remove("SILICO_LOG")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty());
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.starts_with("silico serve: the workspace can't be /, the root of a disk"),
            "{stderr}"
        );
    }
    // Nor started in it.
    let out = Command::new(env!("CARGO_BIN_EXE_silico"))
        .arg("serve")
        .current_dir("/")
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // And a stray argument is a usage error.
    let out = Command::new(env!("CARGO_BIN_EXE_silico"))
        .args(["serve", "--port", "80"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .starts_with("usage: silico check")
    );
}

#[test]
fn serve_ends_on_sigterm_with_its_input_still_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_silico"))
        .args(["serve", "--workspace"])
        .arg(dir.path())
        .env_remove("SILICO_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    // Answered, so it's serving (and its handler registered) before the signal.
    writeln!(stdin, r#"{{"jsonrpc": "2.0", "id": 1, "method": "ping"}}"#).unwrap();
    let mut pong = String::new();
    stdout.read_line(&mut pong).unwrap();
    assert_eq!(pong, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let (done, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait());
    });
    let status = exited
        .recv_timeout(HANG)
        .expect("serve didn't end")
        .unwrap();
    assert_eq!(status.code(), Some(0));
    drop(stdin);
}
